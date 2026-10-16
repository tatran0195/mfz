use super::*;
use std::{sync::atomic::AtomicUsize, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

fn decode(wire: Bytes) -> DecodedFrame {
    match decode_frame(&mut BytesMut::from(&wire[..])) {
        Decoded::Frame(frame) => frame,
        _ => panic!("expected frame"),
    }
}

#[tokio::test]
async fn tiny_frames_survive_original_256_frame_failure() {
    let (session, mut outbound) = SmuxSession::new();
    let mut stream = session.open_stream().await.unwrap();
    outbound.recv().await.unwrap();
    let mut wire = BytesMut::new();
    for i in 0..600u32 {
        wire.extend_from_slice(&encode(CMD_PSH, stream.id(), &i.to_le_bytes()));
    }
    wire.extend_from_slice(&control(CMD_FIN, stream.id()));
    session.dispatch(&mut wire).await.unwrap();
    let mut actual = Vec::new();
    stream.read_to_end(&mut actual).await.unwrap();
    assert_eq!(actual.len(), 2400);
    for (i, chunk) in actual.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(*chunk, (i as u32).to_le_bytes());
    }
}

#[tokio::test]
async fn slow_reader_backpressures_only_at_session_budget_and_loses_nothing() {
    let (session, mut outbound) = SmuxSession::new();
    let mut stream = session.open_stream().await.unwrap();
    outbound.recv().await.unwrap();
    let id = stream.id();
    let producer_session = session.clone();
    let progress = Arc::new(AtomicUsize::new(0));
    let producer_progress = progress.clone();
    let producer = tokio::spawn(async move {
        for i in 0..600usize {
            let payload = vec![(i % 251) as u8; MAX_FRAME_PAYLOAD];
            producer_session
                .dispatch(&mut BytesMut::from(&encode(CMD_PSH, id, &payload)[..]))
                .await
                .unwrap();
            producer_progress.store(i + 1, Ordering::Release);
        }
        producer_session
            .dispatch(&mut BytesMut::from(&control(CMD_FIN, id)[..]))
            .await
            .unwrap();
    });
    tokio::task::yield_now().await;
    assert!(progress.load(Ordering::Acquire) < 600);
    let mut actual = Vec::new();
    timeout(Duration::from_secs(5), stream.read_to_end(&mut actual))
        .await
        .unwrap()
        .unwrap();
    producer.await.unwrap();
    assert_eq!(actual.len(), 600 * MAX_FRAME_PAYLOAD);
    for (i, chunk) in actual.as_chunks::<MAX_FRAME_PAYLOAD>().0.iter().enumerate() {
        assert!(chunk.iter().all(|byte| *byte == (i % 251) as u8));
    }
    assert_eq!(
        session.inner.receive_credit.available_permits(),
        RECEIVE_BUFFER_BYTES
    );
}

#[tokio::test]
async fn stalled_stream_does_not_block_another_stream_below_shared_budget() {
    let (session, mut outbound) = SmuxSession::new();
    let slow = session.open_stream().await.unwrap();
    let mut fast = session.open_stream().await.unwrap();
    outbound.recv().await.unwrap();
    outbound.recv().await.unwrap();
    let mut wire = BytesMut::new();
    for _ in 0..64 {
        wire.extend_from_slice(&encode(CMD_PSH, slow.id(), &vec![1; MAX_FRAME_PAYLOAD]));
    }
    wire.extend_from_slice(&encode(CMD_PSH, fast.id(), b"responsive"));
    timeout(Duration::from_secs(1), session.dispatch(&mut wire))
        .await
        .unwrap()
        .unwrap();
    let mut actual = [0; 10];
    fast.read_exact(&mut actual).await.unwrap();
    assert_eq!(&actual, b"responsive");
    drop(slow);
    assert_eq!(
        session.inner.receive_credit.available_permits(),
        RECEIVE_BUFFER_BYTES
    );
}

#[tokio::test]
async fn round_robin_prevents_bulk_writer_from_monopolizing_output() {
    let (session, mut outbound) = SmuxSession::new();
    let mut bulk = session.open_stream().await.unwrap();
    let mut interactive = session.open_stream().await.unwrap();
    outbound.recv().await.unwrap();
    outbound.recv().await.unwrap();
    bulk.write_all(&vec![7; 2 * MAX_FRAME_PAYLOAD])
        .await
        .unwrap();
    interactive.write_all(b"input").await.unwrap();
    let first = decode(outbound.recv().await.unwrap());
    let second = decode(outbound.recv().await.unwrap());
    let third = decode(outbound.recv().await.unwrap());
    assert_eq!(first.stream_id, bulk.id());
    assert_eq!(second.stream_id, interactive.id());
    assert_eq!(third.stream_id, bulk.id());
    assert_eq!(&second.payload[..], b"input");
}

#[tokio::test]
async fn cancelled_full_queue_write_accepts_no_bytes_and_other_stream_writes() {
    let (session, mut outbound) = SmuxSession::new();
    let mut bulk = session.open_stream().await.unwrap();
    let mut interactive = session.open_stream().await.unwrap();
    outbound.recv().await.unwrap();
    outbound.recv().await.unwrap();
    bulk.write_all(&vec![1; 2 * MAX_FRAME_PAYLOAD])
        .await
        .unwrap();
    assert!(timeout(Duration::from_millis(10), bulk.write(b"cancelled"))
        .await
        .is_err());
    timeout(Duration::from_secs(1), interactive.write_all(b"input"))
        .await
        .unwrap()
        .unwrap();
    for _ in 0..3 {
        assert_ne!(
            &decode(outbound.recv().await.unwrap()).payload[..],
            b"cancelled"
        );
    }
}

#[tokio::test]
async fn fin_on_drop_is_not_lost_or_allowed_to_overtake_full_data_queue() {
    let (session, mut outbound) = SmuxSession::new();
    let mut stream = session.open_stream().await.unwrap();
    let id = stream.id();
    outbound.recv().await.unwrap();
    stream
        .write_all(&vec![9; 2 * MAX_FRAME_PAYLOAD])
        .await
        .unwrap();
    drop(stream);
    assert_eq!(decode(outbound.recv().await.unwrap()).cmd, CMD_PSH);
    assert_eq!(decode(outbound.recv().await.unwrap()).cmd, CMD_PSH);
    let fin = decode(outbound.recv().await.unwrap());
    assert_eq!((fin.cmd, fin.stream_id), (CMD_FIN, id));
    assert!(outbound.try_recv().is_err());
    assert!(session.inner.scheduler.lock().unwrap().streams.is_empty());
}

#[tokio::test]
async fn half_close_retains_read_data_until_remote_fin_and_eof() {
    let (session, mut outbound) = SmuxSession::new();
    let mut stream = session.open_stream().await.unwrap();
    outbound.recv().await.unwrap();
    stream.shutdown().await.unwrap();
    assert_eq!(decode(outbound.recv().await.unwrap()).cmd, CMD_FIN);
    let mut incoming = BytesMut::from(&encode(CMD_PSH, stream.id(), b"tail")[..]);
    incoming.extend_from_slice(&control(CMD_FIN, stream.id()));
    session.dispatch(&mut incoming).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"tail");
    drop(stream);
    assert!(outbound.try_recv().is_err());
}

#[tokio::test]
async fn active_and_closing_streams_and_keepalives_are_bounded() {
    let (session, mut outbound) = SmuxSession::new();
    let mut streams = Vec::new();
    for _ in 0..MAX_STREAMS {
        streams.push(session.open_stream().await.unwrap());
    }
    assert!(session.open_stream().await.is_err());
    drop(streams);
    assert!(session.open_stream().await.is_err()); // cannot accumulate unbounded FIN tombstones.
    for _ in 0..2 * MAX_STREAMS {
        outbound.recv().await.unwrap();
    }
    assert!(session.inner.scheduler.lock().unwrap().streams.is_empty());
    for _ in 0..100 {
        session.send_keepalive().await.unwrap();
    }
    assert_eq!(decode(outbound.recv().await.unwrap()).cmd, CMD_NOP);
    assert!(outbound.try_recv().is_err());
    assert_eq!(
        session.open_stream().await.unwrap().id(),
        (MAX_STREAMS * 2 + 5) as u32
    );
}

#[tokio::test]
async fn close_unblocks_global_receive_credit_and_pending_write() {
    let (session, mut outbound) = SmuxSession::new();
    let mut stream = session.open_stream().await.unwrap();
    outbound.recv().await.unwrap();
    let id = stream.id();
    let frame = encode(CMD_PSH, id, &vec![1; MAX_FRAME_PAYLOAD]);
    let count = RECEIVE_BUFFER_BYTES / (MAX_FRAME_PAYLOAD + CHUNK_OVERHEAD);
    for _ in 0..count {
        session
            .dispatch(&mut BytesMut::from(&frame[..]))
            .await
            .unwrap();
    }
    stream
        .write_all(&vec![2; 2 * MAX_FRAME_PAYLOAD])
        .await
        .unwrap();
    let dispatcher_session = session.clone();
    let dispatcher = tokio::spawn(async move {
        dispatcher_session
            .dispatch(&mut BytesMut::from(&frame[..]))
            .await
    });
    tokio::task::yield_now().await;
    assert!(!dispatcher.is_finished());
    drop(outbound);
    timeout(Duration::from_secs(1), dispatcher)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        stream.write(b"x").await.unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );
    assert_eq!(
        session.inner.receive_credit.available_permits(),
        RECEIVE_BUFFER_BYTES
    );
    let mut bytes = [0];
    assert_eq!(stream.read(&mut bytes).await.unwrap(), 0);
}

#[tokio::test]
async fn closed_session_rejects_open_and_keepalive_and_id_overflow() {
    let (session, _outbound) = SmuxSession::new();
    session.inner.next_id.store(u32::MAX, Ordering::Relaxed);
    assert!(session.open_stream().await.is_err());
    session.close();
    assert!(session.open_stream().await.is_err());
    assert!(session.send_keepalive().await.is_err());
}

#[test]
fn header_layout_matches_smux_v1() {
    let frame = encode(CMD_PSH, 3, b"hello");
    assert_eq!(&frame[..8], &[1, 2, 5, 0, 3, 0, 0, 0]);
    assert_eq!(&frame[8..], b"hello");
}

#[test]
fn frames_round_trip() {
    let mut buf = BytesMut::from(&encode(CMD_PSH, 7, b"smux payload")[..]);
    match decode_frame(&mut buf) {
        Decoded::Frame(frame) => {
            assert_eq!(frame.cmd, CMD_PSH);
            assert_eq!(frame.stream_id, 7);
            assert_eq!(&frame.payload[..], b"smux payload");
        }
        _ => panic!("expected a complete frame"),
    }
    assert!(buf.is_empty());
}

#[test]
fn control_frames_have_no_payload() {
    let mut buf = BytesMut::from(&control(CMD_SYN, 5)[..]);
    match decode_frame(&mut buf) {
        Decoded::Frame(frame) => {
            assert_eq!(frame.cmd, CMD_SYN);
            assert_eq!(frame.stream_id, 5);
            assert!(frame.payload.is_empty());
        }
        _ => panic!("expected a complete frame"),
    }
}

#[test]
fn frames_reassemble_across_arbitrary_chunk_boundaries() {
    let mut wire = BytesMut::new();
    wire.extend_from_slice(&encode(CMD_PSH, 1, b"first"));
    wire.extend_from_slice(&control(CMD_FIN, 1));
    wire.extend_from_slice(&encode(CMD_PSH, 3, b"second"));
    let mut buf = BytesMut::new();
    let mut decoded = Vec::new();
    for byte in wire.iter() {
        buf.extend_from_slice(&[*byte]);
        while let Decoded::Frame(frame) = decode_frame(&mut buf) {
            decoded.push((frame.cmd, frame.stream_id, frame.payload));
        }
    }
    assert_eq!(decoded.len(), 3);
    assert_eq!(&decoded[0].2[..], b"first");
    assert_eq!(decoded[1].0, CMD_FIN);
    assert_eq!(decoded[2].1, 3);
    assert_eq!(&decoded[2].2[..], b"second");
}

#[test]
fn rejects_invalid_commands_and_control_payloads() {
    let mut unknown = BytesMut::from(&control(99, 1)[..]);
    assert!(matches!(decode_frame(&mut unknown), Decoded::Corrupt(_)));
    let mut bad_fin = BytesMut::from(&encode(CMD_FIN, 1, b"x")[..]);
    assert!(matches!(decode_frame(&mut bad_fin), Decoded::Corrupt(_)));
}
