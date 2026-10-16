use super::*;
use bytes::Bytes;
use std::{
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

struct RecordingSink {
    data: mpsc::Sender<Bytes>,
    flushes: Arc<AtomicUsize>,
    block_flush: bool,
}
impl Sink<Message> for RecordingSink {
    type Error = WsError;
    fn poll_ready(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), WsError>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(self: Pin<&mut Self>, message: Message) -> std::result::Result<(), WsError> {
        if let Message::Binary(bytes) = message {
            self.data
                .try_send(bytes)
                .map_err(|_| WsError::Io(std::io::Error::other("test capture full")))?;
        }
        Ok(())
    }
    fn poll_flush(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), WsError>> {
        if self.block_flush {
            return Poll::Pending;
        }
        self.flushes.fetch_add(1, Ordering::Relaxed);
        Poll::Ready(Ok(()))
    }
    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::result::Result<(), WsError>> {
        self.poll_flush(cx)
    }
}
fn packet() -> InputPacket {
    InputPacket {
        payload_type: PAYLOAD_TYPE_DATA,
        payload: Bytes::from_static(b"data"),
    }
}
fn fixture() -> (RecordingSink, mpsc::Receiver<Bytes>, Arc<AtomicUsize>) {
    let (data, rx) = mpsc::channel(256);
    let flushes = Arc::new(AtomicUsize::new(0));
    (
        RecordingSink {
            data,
            flushes: flushes.clone(),
            block_flush: false,
        },
        rx,
        flushes,
    )
}

#[tokio::test]
async fn ready_frames_are_flushed_in_bounded_batches_not_one_by_one() {
    let (sink, mut data, flushes) = fixture();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
    for _ in 0..129 {
        input_tx.send(packet()).await.unwrap();
    }
    let writer = tokio::spawn(websocket_writer(sink, control_rx, input_rx));
    for sequence in 0..129 {
        assert_eq!(
            SsmFrame::decode(&data.recv().await.unwrap())
                .unwrap()
                .sequence_number,
            sequence
        );
    }
    assert_eq!(flushes.load(Ordering::Relaxed), 3);
    drop(control_tx);
    drop(input_tx);
    writer.await.unwrap().unwrap();
}

#[tokio::test]
async fn single_message_does_not_wait_for_batch_to_fill() {
    let (sink, mut data, flushes) = fixture();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
    input_tx.send(packet()).await.unwrap();
    let writer = tokio::spawn(websocket_writer(sink, control_rx, input_rx));
    tokio::time::timeout(Duration::from_secs(1), data.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(flushes.load(Ordering::Relaxed), 1);
    drop(control_tx);
    drop(input_tx);
    writer.await.unwrap().unwrap();
}

#[tokio::test]
async fn continuously_ready_controls_do_not_starve_data() {
    let (sink, mut data, _) = fixture();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    for _ in 0..CONTROL_QUEUE_DEPTH {
        control_tx
            .send(WriterControl::Send(Message::Pong(Bytes::new())))
            .await
            .unwrap();
    }
    let flood_tx = control_tx.clone();
    let flood = tokio::spawn(async move {
        while flood_tx
            .send(WriterControl::Send(Message::Pong(Bytes::new())))
            .await
            .is_ok()
        {}
    });
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
    input_tx.send(packet()).await.unwrap();
    let writer = tokio::spawn(websocket_writer(sink, control_rx, input_rx));
    tokio::time::timeout(Duration::from_secs(1), data.recv())
        .await
        .unwrap()
        .unwrap();
    flood.abort();
    let _ = flood.await;
    drop(control_tx);
    drop(input_tx);
    writer.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn blocked_flush_has_a_deadline_instead_of_hanging_indefinitely() {
    let (mut sink, _data, _) = fixture();
    sink.block_flush = true;
    let (_control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
    input_tx.send(packet()).await.unwrap();
    let error = websocket_writer(sink, control_rx, input_rx)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
}

#[tokio::test]
async fn closed_input_drains_last_ack_without_needing_control_sender_to_close() {
    let (sink, mut data, _) = fixture();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
    input_tx.send(packet()).await.unwrap();
    drop(input_tx);
    let writer = tokio::spawn(websocket_writer(sink, control_rx, input_rx));
    let frame = SsmFrame::decode(&data.recv().await.unwrap()).unwrap();
    control_tx
        .send(WriterControl::Acknowledged(AckPayload::for_frame(&frame)))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), writer)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
