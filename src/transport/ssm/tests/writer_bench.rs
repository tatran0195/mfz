//! Opt-in writer microbenchmark. Same source is run against the previous candidate.
//! This is an in-memory protocol/ACK-RTT benchmark, NOT an AWS throughput claim.
use super::*;
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf},
    time::timeout,
};
use tokio_tungstenite::tungstenite::protocol::Role;

struct CountWrites<T> {
    io: T,
    writes: Arc<AtomicU64>,
}
impl<T: AsyncRead + Unpin> AsyncRead for CountWrites<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, b)
    }
}
impl<T: AsyncWrite + Unpin> AsyncWrite for CountWrites<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.io).poll_write(cx, b);
        if matches!(result, Poll::Ready(Ok(n)) if n > 0) {
            self.writes.fetch_add(1, Ordering::Relaxed);
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[tokio::test]
#[ignore = "opt-in microbenchmark; use --release --ignored --nocapture"]
async fn writer_throughput_matrix() {
    let mib: usize = std::env::var("MFZ_BENCH_MIB")
        .unwrap_or_else(|_| "4".into())
        .parse()
        .unwrap();
    let frames = mib * 1024;
    let label = std::env::var("MFZ_BENCH_LABEL").unwrap_or_else(|_| "current".into());
    let latencies: Vec<u64> = std::env::var("MFZ_BENCH_RTTS")
        .unwrap_or_else(|_| "0,50,250".into())
        .split(',')
        .map(|s| s.parse().unwrap())
        .collect();
    for rtt_ms in latencies {
        let writes = Arc::new(AtomicU64::new(0));
        let (a, b): (DuplexStream, DuplexStream) = tokio::io::duplex(256 * 1024);
        let client = WebSocketStream::from_raw_socket(
            CountWrites {
                io: a,
                writes: writes.clone(),
            },
            Role::Client,
            None,
        )
        .await;
        let mut server = WebSocketStream::from_raw_socket(b, Role::Server, None).await;
        let (sink, _source) = client.split();
        let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
        let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
        let writer = tokio::spawn(websocket_writer(sink, control_rx, input_rx));
        let payload = Bytes::from(vec![0xa5; MAX_STREAM_DATA_PAYLOAD]);
        let start = Instant::now();
        let producer = tokio::spawn(async move {
            for _ in 0..frames {
                input_tx
                    .send(InputPacket {
                        payload_type: PAYLOAD_TYPE_DATA,
                        payload: payload.clone(),
                    })
                    .await
                    .unwrap();
            }
            input_tx
        });
        let remote = tokio::spawn(async move {
            let mut seen = std::collections::HashSet::new();
            let mut acknowledgements = std::collections::VecDeque::new();
            let mut count = 0usize;
            let mut duplicates = 0usize;
            while count < frames {
                let next_due = acknowledgements
                    .front()
                    .map(|(t, _)| *t)
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(60));
                tokio::select! {
                    message = server.next(), if seen.len() < frames => {
                        let Message::Binary(wire) = message.unwrap().unwrap() else {continue;};
                        let frame = SsmFrame::decode(&wire).unwrap();
                        assert_eq!(frame.payload_type,PAYLOAD_TYPE_DATA);
                        assert_eq!(frame.payload.len(),MAX_STREAM_DATA_PAYLOAD);
                        if !seen.insert(frame.sequence_number) {duplicates+=1;}
                        let ack: AckPayload = serde_json::from_value(serde_json::json!({
                            "AcknowledgedMessageType":"input_stream_data",
                            "AcknowledgedMessageId":frame.message_id_str(),
                            "AcknowledgedMessageSequenceNumber":frame.sequence_number,
                            "IsSequentialMessage":true
                        })).unwrap();
                        acknowledgements.push_back((Instant::now()+Duration::from_millis(rtt_ms),ack));
                    }
                    _ = tokio::time::sleep_until(next_due), if !acknowledgements.is_empty() => {
                        let (_,ack) = acknowledgements.pop_front().unwrap();
                        // Count unique original ACKs; duplicates can occur after a retry.
                        if ack.acknowledged_message_sequence_number == count as i64 {count+=1;}
                        control_tx.send(WriterControl::Acknowledged(ack)).await.unwrap();
                    }
                }
            }
            (control_tx, duplicates)
        });
        let input_tx = timeout(Duration::from_secs(90), producer)
            .await
            .unwrap()
            .unwrap();
        let (control_tx, duplicates) = timeout(Duration::from_secs(90), remote)
            .await
            .unwrap()
            .unwrap();
        let elapsed = start.elapsed();
        println!(
            "BENCH {}",
            serde_json::json!({
                "implementation":label,"rtt_ms":rtt_ms,"payload_mib":mib,
                "elapsed_ms":elapsed.as_secs_f64()*1000.0,
                "mib_per_second":mib as f64/elapsed.as_secs_f64(),
                "transport_write_calls":writes.load(Ordering::Relaxed),
                "data_frames":frames,"duplicate_frames":duplicates,
                "scope":"writer microbenchmark; in-memory peer; no TLS or AWS"
            })
        );
        drop(control_tx);
        drop(input_tx);
        timeout(Duration::from_secs(2), writer)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
