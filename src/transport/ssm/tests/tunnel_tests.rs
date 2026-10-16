//! Regression tests for the production native transport; no AWS credentials needed.
use super::*;
use futures_util::SinkExt;
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    time::timeout,
};
use tokio_tungstenite::tungstenite::protocol::Role;

fn output(sequence: i64, payload: Vec<u8>) -> SsmFrame {
    SsmFrame::new(
        msg_type("output_stream_data"),
        sequence,
        FLAG_NONE,
        PAYLOAD_TYPE_DATA,
        payload,
    )
}

fn ack_for(frame: &SsmFrame) -> AckPayload {
    AckPayload {
        acknowledged_message_type: frame.type_name().into(),
        acknowledged_message_id: frame.message_uuid(),
        acknowledged_message_sequence_number: frame.sequence_number,
        is_sequential_message: true,
    }
}

fn packet() -> InputPacket {
    InputPacket {
        payload_type: PAYLOAD_TYPE_DATA,
        payload: Bytes::from_static(b"input"),
    }
}

fn handshake_payload(version: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "AgentVersion": version,
        "RequestedClientActions": [{
            "ActionType": "SessionType",
            "ActionParameters": { "SessionType": "Port", "Properties": {} }
        }]
    }))
    .unwrap()
}

async fn websocket_pair() -> (WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
    let (client, server) = tokio::io::duplex(1024 * 1024);
    tokio::join!(
        WebSocketStream::from_raw_socket(client, Role::Client, None),
        WebSocketStream::from_raw_socket(server, Role::Server, None),
    )
}

async fn next_message<T: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<T>) -> Message {
    timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

fn unused_port() -> u16 {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn send_window_is_bounded_and_only_matching_selective_acks_release_capacity() {
    let now = Instant::now();
    let window_size = 8;
    let mut window = OutgoingWindow::new(TunnelOptions {
        window_messages: window_size,
        ..TunnelOptions::default()
    });
    let first = SsmFrame::decode(&window.prepare(packet(), now).unwrap()).unwrap();
    for _ in 1..window_size {
        window.prepare(packet(), now).unwrap();
    }
    assert!(!window.has_capacity());
    assert!(window.prepare(packet(), now).is_err());
    let mut ack = ack_for(&first);
    ack.acknowledged_message_id = Uuid::new_v4();
    assert!(!window.acknowledge(&ack, now));
    assert!(!window.has_capacity());
    ack = ack_for(&first);
    ack.acknowledged_message_type = "output_stream_data".into();
    assert!(!window.acknowledge(&ack, now));
    ack = ack_for(&first);
    assert!(window.acknowledge(&ack, now));
    assert_eq!(
        window.pending.len(),
        window_size - 1,
        "ACK is selective, not cumulative"
    );
    assert!(
        !window.acknowledge(&ack, now),
        "duplicate ACK must not free another slot"
    );
    let next = SsmFrame::decode(&window.prepare(packet(), now).unwrap()).unwrap();
    assert_eq!(next.sequence_number, window_size as i64);
}

#[test]
fn oversized_input_data_is_rejected_without_consuming_a_sequence() {
    let mut window = OutgoingWindow::default();
    assert!(window
        .prepare(
            InputPacket {
                payload_type: PAYLOAD_TYPE_DATA,
                payload: Bytes::from(vec![0; MAX_STREAM_DATA_PAYLOAD + 1]),
            },
            Instant::now()
        )
        .is_err());
    assert_eq!(window.next_sequence, 0);
    assert!(window.pending.is_empty());
}

#[test]
fn retries_preserve_exact_wire_bytes_and_eventually_fail_instead_of_hanging() {
    let now = Instant::now();
    let mut window = OutgoingWindow::default();
    let original = window.prepare(packet(), now).unwrap();
    assert!(window
        .retry_due(now + INITIAL_RTO - Duration::from_millis(1))
        .unwrap()
        .is_none());
    let mut attempts = 0;
    loop {
        let due = window.next_deadline().unwrap();
        match window.retry_due(due) {
            Ok(Some((sequence, wire))) => {
                assert_eq!(sequence, 0);
                assert_eq!(wire, original);
                attempts += 1;
            }
            Err(_) => break,
            Ok(None) => panic!("indexed deadline must be ready"),
        }
    }
    assert!(attempts > 1 && attempts <= MAX_RETRIES);
    assert!(window.acknowledge(
        &ack_for(&SsmFrame::decode(&original).unwrap()),
        now + MESSAGE_LIFETIME
    ));
    assert!(window
        .retry_due(now + Duration::from_secs(1000))
        .unwrap()
        .is_none());
}

#[test]
fn receive_window_orders_and_deduplicates_without_replaying_bytes() {
    let mut window = IncomingWindow::new(2);
    let four = output(4, b"four".to_vec());
    assert!(window.admit(four.clone()).unwrap());
    assert!(window.pop_ready().is_none());
    assert!(window.admit(output(2, b"two".to_vec())).unwrap());
    assert_eq!(window.pop_ready().unwrap().payload, &b"two"[..]);
    assert!(window.admit(output(2, b"two".to_vec())).unwrap());
    assert!(window.admit(four.clone()).unwrap());
    assert_eq!(window.frames.len(), 1);
    assert!(window.admit(output(3, b"three".to_vec())).unwrap());
    assert_eq!(window.pop_ready().unwrap().payload, &b"three"[..]);
    assert_eq!(window.pop_ready().unwrap().payload, &b"four"[..]);
    assert!(window.pop_ready().is_none());
    assert_eq!(window.bytes, 0);
    assert_eq!(window.expected, 5);
}

#[test]
fn receive_window_reserves_room_for_the_missing_sequence() {
    let mut window = IncomingWindow::new(0);
    for sequence in 1..MAX_INCOMING_FRAMES {
        assert!(window.admit(output(sequence as i64, vec![0; 1])).unwrap());
    }
    assert!(!window
        .admit(output(MAX_INCOMING_FRAMES as i64, vec![0; 1]))
        .unwrap());
    assert!(window
        .admit(output(0, vec![0; MAX_INCOMING_PAYLOAD]))
        .unwrap());
    assert_eq!(window.frames.len(), MAX_INCOMING_FRAMES);
    for sequence in 0..MAX_INCOMING_FRAMES {
        assert_eq!(window.pop_ready().unwrap().sequence_number, sequence as i64);
    }
    assert_eq!(window.bytes, 0);
    assert!(window
        .admit(output(MAX_INCOMING_FRAMES as i64, vec![0; 1]))
        .unwrap());
}

#[test]
fn receive_window_is_bounded_by_bytes_as_well_as_frame_count() {
    let mut window = IncomingWindow::new(0);
    let mut accepted = 0;
    for sequence in 1..100 {
        if window
            .admit(output(sequence, vec![0; MAX_INCOMING_PAYLOAD]))
            .unwrap()
        {
            accepted += 1;
        }
    }
    assert_eq!(
        accepted,
        (MAX_INCOMING_BYTES - MAX_INCOMING_PAYLOAD - HEADER_SIZE)
            / (MAX_INCOMING_PAYLOAD + HEADER_SIZE)
    );
    assert!(window
        .admit(output(0, vec![0; MAX_INCOMING_PAYLOAD]))
        .unwrap());
    assert_eq!(
        window.bytes,
        (accepted + 1) * (MAX_INCOMING_PAYLOAD + HEADER_SIZE)
    );
    assert!(window.bytes <= MAX_INCOMING_BYTES);
    assert!(window
        .admit(output(101, vec![0; MAX_INCOMING_PAYLOAD + 1]))
        .is_err());
}

#[test]
fn malformed_headers_and_corrupt_payloads_are_rejected() {
    let original = output(0, b"verified".to_vec()).encode().to_vec();
    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(SsmFrame::decode(&corrupt).is_none());
    let mut bad_header = original.clone();
    bad_header[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(SsmFrame::decode(&bad_header).is_none());
    let mut bad_schema = original;
    bad_schema[36..40].copy_from_slice(&2u32.to_be_bytes());
    assert!(SsmFrame::decode(&bad_schema).is_none());
}

#[test]
fn publication_controls_and_empty_payloads_allow_unused_metadata() {
    for kind in ["start_publication", "pause_publication"] {
        let mut wire = vec![0u8; HEADER_SIZE];
        wire[4..4 + kind.len()].copy_from_slice(kind.as_bytes());
        let decoded = SsmFrame::decode(&wire).unwrap();
        assert_eq!(decoded.type_name(), kind, "MGS also uses NUL padding");
        assert!(decoded.payload.is_empty());
    }
    let mut empty = output(2, vec![]).encode().to_vec();
    empty[80..112].fill(0);
    assert!(SsmFrame::decode(&empty).is_some());
    assert!(handshake_response(&handshake_payload("3.0.196.0")).is_err());
    assert!(
        handshake_response(&handshake_payload("3.1.1511.0"))
            .unwrap()
            .1
    );
}

#[test]
fn handshake_checks_agent_version_session_type_and_kms_support() {
    assert!(
        !handshake_response(&handshake_payload("3.3.2000.0"))
            .unwrap()
            .1
    );
    assert!(
        handshake_response(&handshake_payload("3.0.197.0"))
            .unwrap()
            .1
    );
    assert!(
        !handshake_response(&handshake_payload("3.1.1512.0"))
            .unwrap()
            .1
    );
    assert!(handshake_response(&handshake_payload("2.3.0.0")).is_err());
    assert!(handshake_response(b"not-json").is_err());
    let mut request: serde_json::Value =
        serde_json::from_slice(&handshake_payload("3.3.2000.0")).unwrap();
    request["RequestedClientActions"][0]["ActionParameters"]["SessionType"] =
        "Standard_Stream".into();
    assert!(handshake_response(&serde_json::to_vec(&request).unwrap()).is_err());
    request["RequestedClientActions"][0]["ActionType"] = "KMSEncryption".into();
    assert!(handshake_response(&serde_json::to_vec(&request).unwrap())
        .unwrap_err()
        .to_string()
        .contains("KMS"));
}

#[tokio::test]
async fn full_input_window_still_services_controls_and_new_data_waits_for_ack() {
    let (client, mut server) = websocket_pair().await;
    let (sink, _source) = client.split();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
    let window_size = 8;
    let writer = tokio::spawn(websocket_writer_with_options(
        sink,
        control_rx,
        input_rx,
        TunnelOptions {
            window_messages: window_size,
            ..TunnelOptions::default()
        },
        None,
    ));
    for _ in 0..window_size + 1 {
        input_tx.send(packet()).await.unwrap();
    }
    let mut first = None;
    for sequence in 0..window_size {
        let Message::Binary(wire) = next_message(&mut server).await else {
            panic!("expected data");
        };
        let frame = SsmFrame::decode(&wire).unwrap();
        assert_eq!(frame.sequence_number, sequence as i64);
        if sequence == 0 {
            first = Some(frame);
        }
    }
    assert!(timeout(Duration::from_millis(20), server.next())
        .await
        .is_err());
    control_tx
        .send(WriterControl::Send(Message::Pong(Bytes::from_static(
            b"alive",
        ))))
        .await
        .unwrap();
    assert_eq!(
        next_message(&mut server).await,
        Message::Pong(Bytes::from_static(b"alive"))
    );
    let mut wrong = ack_for(first.as_ref().unwrap());
    wrong.acknowledged_message_id = Uuid::new_v4();
    control_tx
        .send(WriterControl::Acknowledged(wrong))
        .await
        .unwrap();
    assert!(timeout(Duration::from_millis(20), server.next())
        .await
        .is_err());
    control_tx
        .send(WriterControl::Acknowledged(ack_for(
            first.as_ref().unwrap(),
        )))
        .await
        .unwrap();
    let Message::Binary(wire) = next_message(&mut server).await else {
        panic!("expected data");
    };
    assert_eq!(
        SsmFrame::decode(&wire).unwrap().sequence_number,
        window_size as i64
    );
    drop(control_tx);
    drop(input_tx);
    timeout(Duration::from_secs(1), writer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn websocket_writer_retransmits_an_unacknowledged_frame_unchanged() {
    let (client, mut server) = websocket_pair().await;
    let (sink, _source) = client.split();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
    let writer = tokio::spawn(websocket_writer(sink, control_rx, input_rx));
    input_tx.send(packet()).await.unwrap();
    let first = next_message(&mut server).await;
    assert_eq!(next_message(&mut server).await, first);
    drop(control_tx);
    drop(input_tx);
    timeout(Duration::from_secs(1), writer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn blocked_dispatcher_does_not_block_input_acks_or_pings_and_overflow_is_not_acked() {
    let (message_tx, message_rx) = mpsc::channel(1024);
    let source = futures_util::stream::unfold(message_rx, |mut rx| async move {
        rx.recv().await.map(|message| (Ok(message), rx))
    })
    .boxed();
    let (control_tx, mut control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    let (incoming_tx, mut incoming_rx) = mpsc::channel(1);
    let acknowledged = Arc::new(Mutex::new(Vec::new()));
    let collected = acknowledged.clone();
    let (ping_tx, ping_rx) = oneshot::channel();
    let (ack_tx, ack_rx) = oneshot::channel();
    let collector = tokio::spawn(async move {
        let mut ping_tx = Some(ping_tx);
        let mut ack_tx = Some(ack_tx);
        while let Some(command) = control_rx.recv().await {
            match command {
                WriterControl::Send(Message::Binary(wire)) => {
                    let frame = SsmFrame::decode(&wire).unwrap();
                    let ack: AckPayload = serde_json::from_slice(&frame.payload).unwrap();
                    collected
                        .lock()
                        .unwrap()
                        .push(ack.acknowledged_message_sequence_number);
                }
                WriterControl::Acknowledged(ack) => {
                    if let Some(tx) = ack_tx.take() {
                        tx.send(ack).unwrap();
                    }
                }
                WriterControl::Send(Message::Pong(payload)) => {
                    assert_eq!(&payload[..], b"still alive");
                    ping_tx.take().unwrap().send(()).unwrap();
                }
                _ => panic!("unexpected control"),
            }
        }
    });
    let reader = tokio::spawn(websocket_reader(
        source,
        control_tx,
        incoming_tx,
        IncomingWindow::new(0),
    ));
    let count = MAX_INCOMING_FRAMES + 88;
    let frames: Vec<_> = (0..count)
        .map(|sequence| output(sequence as i64, vec![(sequence % 251) as u8; 1024]))
        .collect();
    for frame in &frames {
        message_tx
            .send(Message::Binary(frame.encode()))
            .await
            .unwrap();
    }
    let input_ack = ack_for(&SsmFrame::new(
        msg_type("input_stream_data"),
        123,
        FLAG_NONE,
        PAYLOAD_TYPE_DATA,
        vec![1],
    ));
    let ack_frame = SsmFrame::new(
        msg_type("acknowledge"),
        0,
        FLAG_ACK,
        PAYLOAD_TYPE_NONE,
        serde_json::to_vec(&input_ack).unwrap(),
    );
    message_tx
        .send(Message::Binary(ack_frame.encode()))
        .await
        .unwrap();
    message_tx
        .send(Message::Ping(Bytes::from_static(b"still alive")))
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), ack_rx)
            .await
            .unwrap()
            .unwrap()
            .acknowledged_message_sequence_number,
        123
    );
    timeout(Duration::from_secs(2), ping_rx)
        .await
        .unwrap()
        .unwrap();
    let retained = acknowledged.lock().unwrap().len();
    assert!(retained <= MAX_INCOMING_FRAMES + 1);
    assert!(
        retained < count,
        "overflow must withhold ACK instead of losing acknowledged bytes"
    );
    for sequence in 0..retained {
        let frame = timeout(Duration::from_secs(2), incoming_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.sequence_number, sequence as i64);
        assert_eq!(frame.payload, vec![(sequence % 251) as u8; 1024]);
    }
    // Now the consumer has capacity. Previously unacknowledged frames can be retried.
    for (sequence, frame) in frames.iter().enumerate().skip(retained) {
        message_tx
            .send(Message::Binary(frame.encode()))
            .await
            .unwrap();
        let frame = timeout(Duration::from_secs(2), incoming_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(frame.sequence_number, sequence as i64);
    }
    drop(message_tx);
    reader.await.unwrap().unwrap();
    collector.await.unwrap();
    assert_eq!(acknowledged.lock().unwrap().len(), count);
}

#[tokio::test]
async fn start_publication_alone_does_not_signal_listener_ready() {
    let (client, mut server) = websocket_pair().await;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (bound_tx, mut bound_rx) = oneshot::channel();
    let native = tokio::spawn(relay_websocket(
        client,
        "test-token".into(),
        unused_port(),
        shutdown_rx,
        bound_tx,
    ));
    assert!(matches!(next_message(&mut server).await, Message::Text(_)));
    server
        .send(Message::Binary(
            SsmFrame::new(msg_type("start_publication"), 0, FLAG_NONE, 0, vec![]).encode(),
        ))
        .await
        .unwrap();
    assert!(timeout(Duration::from_millis(100), &mut bound_rx)
        .await
        .is_err());
    shutdown_tx.send(()).unwrap();
    timeout(Duration::from_secs(1), native)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(bound_rx.await.is_err());
}

#[derive(Default, Debug)]
struct MockStats {
    bytes: usize,
    streams: usize,
    dropped_once: bool,
}

/// A protocol peer, not a second copy of the MFZ relay. It selectively ACKs
/// client data, reassembles fragmented smux frames, and echoes application bytes.
async fn mock_agent<T: AsyncRead + AsyncWrite + Unpin>(
    mut ws: WebSocketStream<T>,
    ack_data: bool,
    echo: bool,
    drop_sequence: Option<i64>,
) -> MockStats {
    let Message::Text(token) = next_message(&mut ws).await else {
        panic!("expected auth token");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&token).unwrap()["TokenValue"],
        "test-token"
    );
    ws.send(Message::Binary(
        SsmFrame::new(msg_type("start_publication"), 0, FLAG_NONE, 0, vec![]).encode(),
    ))
    .await
    .unwrap();
    ws.send(Message::Binary(
        SsmFrame::new(
            msg_type("output_stream_data"),
            0,
            FLAG_NONE,
            PAYLOAD_TYPE_HANDSHAKE_REQ,
            handshake_payload("3.3.2000.0"),
        )
        .encode(),
    ))
    .await
    .unwrap();
    loop {
        let Message::Binary(wire) = next_message(&mut ws).await else {
            continue;
        };
        let frame = SsmFrame::decode(&wire).unwrap();
        if frame.type_name() == "input_stream_data" {
            assert_eq!(frame.sequence_number, 0);
            assert_eq!(frame.payload_type, PAYLOAD_TYPE_HANDSHAKE_RESP);
            ws.send(acknowledgement(&frame).unwrap()).await.unwrap();
            break;
        }
    }
    ws.send(Message::Binary(
        SsmFrame::new(
            msg_type("output_stream_data"),
            1,
            FLAG_NONE,
            PAYLOAD_TYPE_HANDSHAKE_COMPLETE,
            b"{}".to_vec(),
        )
        .encode(),
    ))
    .await
    .unwrap();
    let mut incoming = IncomingWindow::new(1);
    let mut smux_buffer = BytesMut::new();
    let mut output_sequence = 2;
    let mut stats = MockStats::default();
    while let Some(Ok(message)) = ws.next().await {
        let Message::Binary(wire) = message else {
            continue;
        };
        let frame = SsmFrame::decode(&wire).unwrap();
        if frame.type_name() != "input_stream_data" {
            continue;
        }
        assert_eq!(frame.payload_type, PAYLOAD_TYPE_DATA);
        assert!(frame.payload.len() <= MAX_STREAM_DATA_PAYLOAD);
        if drop_sequence == Some(frame.sequence_number) && !stats.dropped_once {
            stats.dropped_once = true;
            continue;
        }
        let ack = acknowledgement(&frame).unwrap();
        if incoming.admit(frame).unwrap() && ack_data && ws.send(ack).await.is_err() {
            break;
        }
        while let Some(frame) = incoming.pop_ready() {
            smux_buffer.extend_from_slice(&frame.payload);
            loop {
                let decoded = match crate::smux::decode_frame(&mut smux_buffer) {
                    crate::smux::Decoded::Frame(frame) => frame,
                    crate::smux::Decoded::Incomplete => break,
                    crate::smux::Decoded::Corrupt(error) => {
                        panic!("mock received corrupt smux: {error}")
                    }
                };
                let response = match decoded.cmd {
                    crate::smux::CMD_SYN => {
                        assert_eq!(decoded.stream_id % 2, 1);
                        stats.streams += 1;
                        None
                    }
                    crate::smux::CMD_PSH => {
                        stats.bytes += decoded.payload.len();
                        echo.then(|| {
                            crate::smux::encode(
                                crate::smux::CMD_PSH,
                                decoded.stream_id,
                                &decoded.payload,
                            )
                        })
                    }
                    crate::smux::CMD_FIN => {
                        echo.then(|| crate::smux::control(crate::smux::CMD_FIN, decoded.stream_id))
                    }
                    crate::smux::CMD_NOP => None,
                    _ => unreachable!(),
                };
                if let Some(response) = response {
                    // Non-aligned splits exercise both smux header and payload reassembly.
                    for chunk in response.chunks(997) {
                        if ws
                            .send(Message::Binary(
                                output(output_sequence, chunk.to_vec()).encode(),
                            ))
                            .await
                            .is_err()
                        {
                            return stats;
                        }
                        output_sequence += 1;
                    }
                }
            }
        }
    }
    stats
}

#[tokio::test]
async fn multi_megabyte_bidirectional_copies_survive_a_lost_ssm_input_frame() {
    let (client, server) = websocket_pair().await;
    let port = unused_port();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (bound_tx, bound_rx) = oneshot::channel();
    let native = tokio::spawn(relay_websocket(
        client,
        "test-token".into(),
        port,
        shutdown_rx,
        bound_tx,
    ));
    let agent = tokio::spawn(mock_agent(server, true, true, Some(4)));
    timeout(Duration::from_secs(5), bound_rx)
        .await
        .unwrap()
        .unwrap();
    let mut clients = JoinSet::new();
    for seed in [17usize, 53] {
        clients.spawn(async move {
            let tcp = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .unwrap();
            let (mut read, mut write) = tcp.into_split();
            let expected: Vec<u8> = (0..1024 * 1024)
                .map(|i| ((i * seed + i / 997) % 251) as u8)
                .collect();
            let mut received = Vec::new();
            let (sent, got) = tokio::join!(
                async {
                    write.write_all(&expected).await?;
                    write.shutdown().await
                },
                read.read_to_end(&mut received),
            );
            sent.unwrap();
            got.unwrap();
            assert_eq!(
                received, expected,
                "the complete byte stream must be identical"
            );
        });
    }
    timeout(Duration::from_secs(30), async {
        while let Some(result) = clients.join_next().await {
            result.unwrap();
        }
    })
    .await
    .unwrap();
    shutdown_tx.send(()).unwrap();
    timeout(Duration::from_secs(2), native)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let stats = timeout(Duration::from_secs(2), agent)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stats.streams, 2);
    assert_eq!(stats.bytes, 2 * 1024 * 1024);
    assert!(stats.dropped_once);
    // Child tasks must not retain the local listener after the tunnel finishes.
    TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
}

// tungstenite fixes this callback's ErrorResponse type; it cannot be boxed.
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn stopping_public_handle_closes_backpressured_tasks_and_tcp_sockets() {
    let websocket_listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let websocket_addr = websocket_listener.local_addr().unwrap();
    let agent =
        tokio::spawn(async move {
            let (tcp, _) = websocket_listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_hdr_async(tcp, |
            request: &tokio_tungstenite::tungstenite::handshake::server::Request,
            response: tokio_tungstenite::tungstenite::handshake::server::Response,
        | {
            let authorization = request.headers().get("authorization").unwrap().to_str().unwrap();
            assert!(authorization.starts_with("AWS4-HMAC-SHA256 "));
            assert!(authorization.contains("/ap-northeast-1/ssmmessages/aws4_request"));
            assert_eq!(request.headers().get("x-amz-security-token").unwrap(), "dummy-sts-token");
            Ok(response)
        }).await.unwrap();
            mock_agent(ws, false, false, None).await
        });
    let port = unused_port();
    // Dummy credentials sign a local mock upgrade; no requests go to AWS.
    let mut handle = start_port_forward_tunnel(
        format!("ws://{websocket_addr}/v1/data-channel/test-session"),
        "test-token".into(),
        port,
        3389,
        SessionCredentials {
            credentials: aws_credential_types::Credentials::new(
                "AKIDEXAMPLE",
                "dummy-secret-for-local-test-only",
                Some("dummy-sts-token".into()),
                None,
                "mfz-local-test",
            ),
            region: "ap-northeast-1".into(),
        },
    )
    .await
    .unwrap();
    timeout(Duration::from_secs(5), handle.wait_until_ready())
        .await
        .unwrap()
        .unwrap();
    let mut tcp = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let upload = tokio::spawn(async move { tcp.write_all(&vec![0x5a; 64 * 1024 * 1024]).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !upload.is_finished(),
        "no-ACK bulk transfer must backpressure the TCP sender"
    );
    handle.stop();
    assert!(
        timeout(Duration::from_secs(2), handle.task.as_mut().unwrap())
            .await
            .unwrap()
            .unwrap_err()
            .is_cancelled()
    );
    assert!(!handle.is_running());
    assert!(timeout(Duration::from_secs(2), upload)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    timeout(Duration::from_secs(2), agent)
        .await
        .unwrap()
        .unwrap();
    TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
}
// tungstenite fixes this callback's ErrorResponse type; it cannot be boxed.
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn async_shutdown_awaits_backpressured_tasks_and_releases_the_port() {
    let websocket_listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let websocket_addr = websocket_listener.local_addr().unwrap();
    let agent =
        tokio::spawn(async move {
            let (tcp, _) = websocket_listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_hdr_async(tcp, |
            request: &tokio_tungstenite::tungstenite::handshake::server::Request,
            response: tokio_tungstenite::tungstenite::handshake::server::Response,
        | {
            let authorization = request.headers().get("authorization").unwrap().to_str().unwrap();
            assert!(authorization.starts_with("AWS4-HMAC-SHA256 "));
            assert!(authorization.contains("/ap-northeast-1/ssmmessages/aws4_request"));
            assert_eq!(request.headers().get("x-amz-security-token").unwrap(), "dummy-sts-token");
            Ok(response)
        }).await.unwrap();
            mock_agent(ws, false, false, None).await
        });
    let port = unused_port();
    // Dummy credentials sign a local mock upgrade; no requests go to AWS.
    let mut handle = start_port_forward_tunnel(
        format!("ws://{websocket_addr}/v1/data-channel/test-session"),
        "test-token".into(),
        port,
        3389,
        SessionCredentials {
            credentials: aws_credential_types::Credentials::new(
                "AKIDEXAMPLE",
                "dummy-secret-for-local-test-only",
                Some("dummy-sts-token".into()),
                None,
                "mfz-local-test",
            ),
            region: "ap-northeast-1".into(),
        },
    )
    .await
    .unwrap();
    timeout(Duration::from_secs(5), handle.wait_until_ready())
        .await
        .unwrap()
        .unwrap();
    let mut tcp = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let upload = tokio::spawn(async move { tcp.write_all(&vec![0x5a; 64 * 1024 * 1024]).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !upload.is_finished(),
        "no-ACK bulk transfer must backpressure the TCP sender"
    );
    timeout(Duration::from_secs(2), handle.shutdown())
        .await
        .unwrap()
        .unwrap();
    handle.shutdown().await.unwrap(); // idempotent; never await a completed JoinHandle twice.
    timeout(Duration::from_secs(1), handle.closed())
        .await
        .unwrap();
    assert!(!handle.is_running());
    assert!(timeout(Duration::from_secs(2), upload)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    timeout(Duration::from_secs(2), agent)
        .await
        .unwrap()
        .unwrap();
    TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
}

#[tokio::test]
async fn saturated_outgoing_ack_queue_cannot_hide_incoming_input_credit() {
    let (message_tx, message_rx) = mpsc::channel(16);
    let source = futures_util::stream::unfold(message_rx, |mut rx| async move {
        rx.recv().await.map(|message| (Ok(message), rx))
    })
    .boxed();
    let (control_tx, _control_rx) = mpsc::channel(1);
    control_tx
        .send(WriterControl::Send(Message::Pong(Bytes::new())))
        .await
        .unwrap();
    let (incoming_tx, _incoming_rx) = mpsc::channel(1);
    let (ack_tx, mut ack_rx) = mpsc::channel(1);
    let reader = tokio::spawn(websocket_reader_with_acks(
        source,
        control_tx,
        incoming_tx,
        IncomingWindow::new(0),
        Some(ack_tx),
    ));
    for sequence in 0..6 {
        message_tx
            .send(Message::Binary(output(sequence, vec![1; 1024]).encode()))
            .await
            .unwrap();
    }
    let frame = SsmFrame::new(
        msg_type("input_stream_data"),
        123,
        FLAG_NONE,
        PAYLOAD_TYPE_DATA,
        Bytes::new(),
    );
    let ack = ack_for(&frame);
    message_tx
        .send(acknowledgement(&frame).unwrap())
        .await
        .unwrap();
    let received = timeout(Duration::from_secs(1), ack_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        received.acknowledged_message_id,
        ack.acknowledged_message_id
    );
    assert_eq!(received.acknowledged_message_sequence_number, 123);
    drop(message_tx);
    reader.await.unwrap().unwrap();
}

fn dummy_credentials() -> SessionCredentials {
    SessionCredentials {
        credentials: aws_credential_types::Credentials::new(
            "AKIDEXAMPLE",
            "dummy",
            Some("dummy".into()),
            None,
            "mfz-local-test",
        ),
        region: "eu-west-1".into(),
    }
}
#[tokio::test]
async fn public_readiness_surfaces_startup_error_and_does_not_treat_liveness_as_ready() {
    let mut handle = start_port_forward_tunnel(
        "not a websocket URL".into(),
        "dummy".into(),
        unused_port(),
        3389,
        dummy_credentials(),
    )
    .await
    .unwrap();
    let error = timeout(Duration::from_secs(2), handle.wait_until_ready())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("Invalid stream URL"));
    assert!(!handle.is_running());
    assert!(handle.wait_until_ready().await.is_err());
    handle.shutdown().await.unwrap();
    timeout(Duration::from_secs(1), handle.closed())
        .await
        .unwrap();
}
fn unresponsive_handle() -> SsmTunnelHandle {
    let (shutdown_tx, _) = oneshot::channel();
    let (_bound, bound_rx) = oneshot::channel();
    let closed = CancellationToken::new();
    let completed = closed.clone().drop_guard();
    let task = tokio::spawn(async move {
        let _completed = completed;
        std::future::pending::<Result<()>>().await
    });
    SsmTunnelHandle {
        shutdown_tx: Some(shutdown_tx),
        bound_rx: Some(bound_rx),
        task: Some(task),
        closed,
        ready: false,
        exit_error: None,
    }
}
#[tokio::test]
async fn cancelling_shutdown_keeps_parent_owned_so_drop_can_abort_it() {
    let mut handle = unresponsive_handle();
    let closed = handle.closed_token();
    assert!(timeout(Duration::from_millis(10), handle.shutdown())
        .await
        .is_err());
    assert!(
        handle.task.is_some(),
        "cancelled shutdown detached the parent"
    );
    drop(handle);
    timeout(Duration::from_secs(1), closed.cancelled())
        .await
        .unwrap();
}
#[tokio::test]
async fn abort_before_first_poll_still_signals_transport_completion() {
    let mut handle = unresponsive_handle();
    let closed = handle.closed_token();
    handle.stop();
    timeout(Duration::from_secs(1), closed.cancelled())
        .await
        .unwrap();
}
#[tokio::test(start_paused = true)]
async fn unresponsive_parent_has_a_bounded_shutdown_fallback() {
    let mut handle = unresponsive_handle();
    assert!(handle
        .shutdown()
        .await
        .unwrap_err()
        .to_string()
        .contains("shutdown timed out"));
    assert!(!handle.is_running());
    handle.closed().await;
}

#[test]
fn sigv4_omits_security_token_for_long_lived_keys_and_preserves_sts_tokens() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    for token in [None, Some("dummy-sts-token")] {
        let credentials = SessionCredentials {
            credentials: aws_credential_types::Credentials::new(
                "AKIDEXAMPLE",
                "dummy-local-secret",
                token.map(str::to_owned),
                None,
                "mfz-local-test",
            ),
            region: "eu-west-1".into(),
        };
        let mut request = "wss://ssmmessages.eu-west-1.amazonaws.com/v1/data-channel/local-test"
            .into_client_request()
            .unwrap();
        sign_request_sigv4(&mut request, &credentials).unwrap();
        assert!(request.headers()["authorization"]
            .to_str()
            .unwrap()
            .contains("/eu-west-1/ssmmessages/aws4_request"));
        assert_eq!(
            request
                .headers()
                .get("x-amz-security-token")
                .map(|value| value.to_str().unwrap()),
            token
        );
    }
}
