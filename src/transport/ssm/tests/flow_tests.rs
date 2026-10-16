use super::*;
fn packet() -> InputPacket {
    InputPacket {
        payload_type: PAYLOAD_TYPE_DATA,
        payload: Bytes::from_static(b"data"),
    }
}
fn ack(wire: &Bytes) -> AckPayload {
    AckPayload::for_frame(&SsmFrame::decode(wire).unwrap())
}

#[test]
fn selective_future_acks_do_not_allow_unbounded_sending_past_a_gap() {
    let now = Instant::now();
    let mut window = OutgoingWindow::new(TunnelOptions {
        window_messages: 4,
        ..TunnelOptions::default()
    });
    let packets: Vec<_> = (0..4)
        .map(|_| window.prepare(packet(), now).unwrap())
        .collect();
    for wire in &packets[1..] {
        assert!(window.acknowledge(&ack(wire), now + Duration::from_millis(50)));
    }
    assert_eq!(window.pending.len(), 1);
    assert!(!window.has_capacity());
    assert!(window.prepare(packet(), now).is_err());
    let (sequence, retry) = window
        .retry_due(now + Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(sequence, 0);
    assert_eq!(retry, packets[0]);
    assert!(window.acknowledge(&ack(&packets[0]), now + Duration::from_secs(1)));
    assert!(window.has_capacity());
    assert!(window.deadlines.is_empty());
    assert_eq!(window.bytes, 0);
    assert_eq!(
        SsmFrame::decode(&window.prepare(packet(), now).unwrap())
            .unwrap()
            .sequence_number,
        4
    );
}

#[test]
fn rtt_updates_are_adaptive_bounded_and_retransmissions_obey_karns_rule() {
    let now = Instant::now();
    let mut window = OutgoingWindow::default();
    let wire = window.prepare(packet(), now).unwrap();
    window.retry_due(now + INITIAL_RTO).unwrap().unwrap();
    assert!(window.acknowledge(&ack(&wire), now + INITIAL_RTO + Duration::from_millis(20)));
    assert_eq!(
        window.rto(),
        INITIAL_RTO,
        "ambiguous retransmission RTT is not a sample"
    );
    let sent = now + Duration::from_secs(2);
    let wire = window.prepare(packet(), sent).unwrap();
    assert!(window.acknowledge(&ack(&wire), sent + Duration::from_millis(50)));
    assert!((150..=160).contains(&window.rto().as_millis()));
    for _ in 0..20 {
        let wire = window.prepare(packet(), sent).unwrap();
        window.acknowledge(&ack(&wire), sent + Duration::from_millis(50));
    }
    assert_eq!(window.rto(), MIN_RTO);
    assert!(window.deadlines.is_empty());
    let mut slow = OutgoingWindow::default();
    let wire = slow.prepare(packet(), now).unwrap();
    slow.acknowledge(&ack(&wire), now + Duration::from_secs(100));
    assert_eq!(slow.rto(), MAX_RTO);
}

#[test]
fn exponential_backoff_is_capped_and_deadline_index_never_keeps_stale_entries() {
    let now = Instant::now();
    let mut window = OutgoingWindow::default();
    let wire = window.prepare(packet(), now).unwrap();
    let mut previous = INITIAL_RTO;
    for _ in 0..10 {
        let due = window.next_deadline().unwrap();
        window.retry_due(due).unwrap().unwrap();
        let delay = window.next_deadline().unwrap() - due;
        assert!(delay >= previous && delay <= MAX_RETRY_DELAY);
        previous = delay;
        assert_eq!(window.deadlines.len(), window.pending.len());
    }
    window.acknowledge(&ack(&wire), now + Duration::from_secs(100));
    assert!(window.deadlines.is_empty());
    assert_eq!(window.bytes, 0);
}

#[test]
fn rtt_clock_starts_at_flush_instead_of_queue_time() {
    let now = Instant::now();
    let mut window = OutgoingWindow::default();
    let wire = window.prepare(packet(), now).unwrap();
    window.mark_flushed(&[0], now + Duration::from_secs(2));
    assert_eq!(
        window.next_deadline().unwrap(),
        now + Duration::from_secs(2) + INITIAL_RTO
    );
    window.acknowledge(
        &ack(&wire),
        now + Duration::from_secs(2) + Duration::from_millis(10),
    );
    assert_eq!(window.rto(), MIN_RTO);
}

#[test]
fn options_fail_fast_instead_of_creating_zero_or_unbounded_queues() {
    assert_eq!(TunnelOptions::default().window_messages, 8192);
    for options in [
        TunnelOptions {
            window_messages: 0,
            ..TunnelOptions::default()
        },
        TunnelOptions {
            window_messages: 65537,
            ..TunnelOptions::default()
        },
        TunnelOptions {
            write_batch_messages: 0,
            ..TunnelOptions::default()
        },
        TunnelOptions {
            write_batch_messages: 257,
            ..TunnelOptions::default()
        },
    ] {
        assert!(options.validate().is_err());
    }
}

#[test]
fn conflicting_buffered_replay_and_overflowing_sequence_are_rejected() {
    let mut window = IncomingWindow::new(0);
    let frame = SsmFrame::new(
        msg_type("output_stream_data"),
        1,
        FLAG_NONE,
        PAYLOAD_TYPE_DATA,
        Bytes::from_static(b"one"),
    );
    window.admit(frame.clone()).unwrap();
    window.admit(frame.clone()).unwrap();
    let mut conflicting = frame;
    conflicting.payload = Bytes::from_static(b"two");
    assert!(window.admit(conflicting).is_err());
    for sequence in [-1, i64::MAX] {
        assert!(window
            .admit(SsmFrame::new(
                msg_type("output_stream_data"),
                sequence,
                FLAG_NONE,
                PAYLOAD_TYPE_DATA,
                Bytes::new()
            ))
            .is_err());
    }
}

#[test]
fn ack_silence_uses_bounded_gap_probes_instead_of_replaying_the_entire_window() {
    let now = Instant::now();
    let mut window = OutgoingWindow::default();
    let packets: Vec<_> = (0..256)
        .map(|_| window.prepare(packet(), now).unwrap())
        .collect();
    for sequence in 0..MAX_RETRY_PROBES {
        assert_eq!(
            window.retry_due(now + INITIAL_RTO).unwrap().unwrap().0,
            sequence as i64
        );
    }
    assert!(window.retry_due(now + INITIAL_RTO).unwrap().is_none());
    assert!(window.next_deadline().unwrap() > now + INITIAL_RTO);
    assert_eq!(window.retransmissions, MAX_RETRY_PROBES as u64);
    window.acknowledge(
        &ack(&packets[0]),
        now + INITIAL_RTO + Duration::from_millis(1),
    );
    assert_eq!(
        window
            .retry_due(now + INITIAL_RTO + Duration::from_millis(1))
            .unwrap()
            .unwrap()
            .0,
        MAX_RETRY_PROBES as i64
    );
    assert_eq!(window.probes.len(), MAX_RETRY_PROBES);
}
