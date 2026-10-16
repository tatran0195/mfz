//! Single-owner, bounded-batch WebSocket writer. No sleep-per-frame throttle,
//! no sink mutex, no per-frame flush, no polling retry interval or biased select.
use super::{
    flow::{InputPacket, OutgoingWindow, TunnelOptions},
    protocol::*,
};
use crate::error::{AppError, Result};
use futures_util::{Sink, SinkExt};
use std::{collections::VecDeque, time::Duration};
use tokio::{sync::mpsc, time::Instant};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tracing::{debug, trace};

pub(crate) const INPUT_QUEUE_DEPTH: usize = 256;
pub(crate) const CONTROL_QUEUE_DEPTH: usize = 512;
pub(crate) const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(15);
const CONTROL_BURST: usize = 128;
const RETRY_BURST: usize = 8;

pub(crate) enum WriterControl {
    Send(Message),
    Acknowledged(AckPayload),
}

pub(crate) fn acknowledgement(frame: &SsmFrame) -> Result<Message> {
    acknowledgement_payload(AckPayload::for_frame(frame))
}
fn acknowledgement_payload(ack: AckPayload) -> Result<Message> {
    let payload = serde_json::to_vec(&ack)
        .map_err(|error| AppError::Ssm(format!("ACK serialization failed: {error}")))?;
    Ok(Message::Binary(
        SsmFrame::new(
            msg_type("acknowledge"),
            0,
            FLAG_ACK,
            PAYLOAD_TYPE_NONE,
            payload,
        )
        .encode(),
    ))
}
pub(crate) fn try_acknowledgement(
    tx: &mpsc::Sender<WriterControl>,
    ack: AckPayload,
) -> Result<bool> {
    match tx.try_reserve() {
        Ok(permit) => {
            permit.send(WriterControl::Send(acknowledgement_payload(ack)?));
            Ok(true)
        }
        Err(mpsc::error::TrySendError::Full(_)) => Ok(false),
        Err(mpsc::error::TrySendError::Closed(_)) => {
            Err(AppError::Ssm("SSM WebSocket writer stopped".into()))
        }
    }
}
pub(crate) async fn control(
    tx: &mpsc::Sender<WriterControl>,
    command: WriterControl,
) -> Result<()> {
    tx.send(command)
        .await
        .map_err(|_| AppError::Ssm("SSM WebSocket writer stopped".into()))
}
/// Ordinary output ACK/Pong is best-effort when the wire queue is saturated.
/// We still own the data; its sender safely retries. Never block the reader here
/// and prevent it seeing the INPUT ACKs that free the other direction's window.
pub(crate) fn try_control(
    tx: &mpsc::Sender<WriterControl>,
    command: WriterControl,
) -> Result<bool> {
    match tx.try_send(command) {
        Ok(()) => Ok(true),
        Err(mpsc::error::TrySendError::Full(_)) => Ok(false),
        Err(mpsc::error::TrySendError::Closed(_)) => {
            Err(AppError::Ssm("SSM WebSocket writer stopped".into()))
        }
    }
}

pub(crate) async fn send_ws<S>(sink: &mut S, message: Message) -> Result<()>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    tokio::time::timeout(SOCKET_WRITE_TIMEOUT, sink.send(message))
        .await
        .map_err(|_| AppError::Ssm("SSM WebSocket write timed out".into()))?
        .map_err(|error| AppError::Ssm(format!("SSM WebSocket send failed: {error}")))
}

#[cfg(test)]
pub(crate) async fn websocket_writer<S>(
    sink: S,
    control_rx: mpsc::Receiver<WriterControl>,
    input_rx: mpsc::Receiver<InputPacket>,
) -> Result<()>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    websocket_writer_with_options(sink, control_rx, input_rx, TunnelOptions::default(), None).await
}

pub(crate) async fn websocket_writer_with_options<S>(
    mut sink: S,
    mut control_rx: mpsc::Receiver<WriterControl>,
    mut input_rx: mpsc::Receiver<InputPacket>,
    options: TunnelOptions,
    mut ack_rx: Option<mpsc::Receiver<AckPayload>>,
) -> Result<()>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    options.validate()?;
    let mut outgoing = OutgoingWindow::new(options);
    let mut first_control = None;
    let mut first_ack = None;
    let mut first_packet = None;
    let mut input_closed = false;
    let mut batch =
        VecDeque::with_capacity(CONTROL_BURST + RETRY_BURST + options.write_batch_messages);
    let mut sequences = Vec::with_capacity(RETRY_BURST + options.write_batch_messages);
    let mut frames_sent = 0u64;
    let mut flushes = 0u64;
    let result = 'outer: loop {
        // Dedicated input-ACK storage is scalar and sized for the whole window;
        // a full outgoing ACK wire queue must not stop incoming credit processing.
        for _ in 0..CONTROL_BURST {
            let ack = first_ack
                .take()
                .or_else(|| ack_rx.as_mut().and_then(|rx| rx.try_recv().ok()));
            if let Some(ack) = ack {
                outgoing.acknowledge(&ack, Instant::now());
            } else {
                break;
            }
        }
        // Quotas give ACK/Pong traffic priority without starving new data forever.
        for _ in 0..CONTROL_BURST {
            let command = first_control.take().or_else(|| control_rx.try_recv().ok());
            match command {
                Some(WriterControl::Send(message)) => batch.push_back(message),
                Some(WriterControl::Acknowledged(ack)) => {
                    if !outgoing.acknowledge(&ack, Instant::now()) {
                        trace!(
                            "SSM: stale/unmatched ACK seq={}",
                            ack.acknowledged_message_sequence_number
                        );
                    }
                }
                None => break,
            }
        }
        for _ in 0..RETRY_BURST {
            match outgoing.retry_due(Instant::now()) {
                Ok(Some((sequence, wire))) => {
                    sequences.push(sequence);
                    batch.push_back(Message::Binary(wire));
                }
                Ok(None) => break,
                Err(error) => break 'outer Err(error),
            }
        }
        for _ in 0..options.write_batch_messages {
            if !outgoing.has_capacity() {
                break;
            }
            let packet = first_packet.take().or_else(|| input_rx.try_recv().ok());
            let Some(packet) = packet else {
                break;
            };
            let sequence = outgoing.next_sequence;
            match outgoing.prepare(packet, Instant::now()) {
                Ok(wire) => {
                    sequences.push(sequence);
                    batch.push_back(Message::Binary(wire));
                }
                Err(error) => break 'outer Err(error),
            }
        }
        if !batch.is_empty() {
            let count = batch.len() as u64;
            let send = async {
                while let Some(message) = batch.pop_front() {
                    sink.feed(message).await?;
                }
                sink.flush().await
            };
            match tokio::time::timeout(SOCKET_WRITE_TIMEOUT, send).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    break Err(AppError::Ssm(format!(
                        "SSM WebSocket batch failed: {error}"
                    )))
                }
                Err(_) => break Err(AppError::Ssm("SSM WebSocket batch write timed out".into())),
            }
            outgoing.mark_flushed(&sequences, Instant::now());
            sequences.clear();
            frames_sent += count;
            flushes += 1;
            // Cooperative scheduling once per bounded batch, not a wall-clock rate cap.
            tokio::task::yield_now().await;
            continue;
        }
        if input_closed && outgoing.pending.is_empty() && first_packet.is_none() {
            break Ok(());
        }
        // ACK-only work can release credit but produce no wire frames. Poll only
        // real channel readiness / the earliest indexed retry deadline while idle.
        let deadline = outgoing.next_deadline();
        tokio::select! {
            command=control_rx.recv()=>match command {
                Some(command)=>first_control=Some(command),
                None=>break Ok(()),
            },
            ack=async {ack_rx.as_mut().expect("guarded ACK inbox").recv().await}, if ack_rx.is_some()=>match ack {
                Some(ack)=>first_ack=Some(ack),
                None=>ack_rx=None,
            },
            packet=input_rx.recv(), if outgoing.has_capacity() && !input_closed=>match packet {
                Some(packet)=>first_packet=Some(packet),
                None=>input_closed=true,
            },
            _=tokio::time::sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some()=>{},
        }
        if input_closed && outgoing.pending.is_empty() && first_packet.is_none() {
            break Ok(());
        }
    };
    debug!(
        frames_sent,
        flushes,
        retransmissions = outgoing.retransmissions,
        peak_pending = outgoing.peak_messages,
        peak_wire_bytes = outgoing.peak_bytes,
        rto_ms = outgoing.rto().as_millis(),
        "SSM writer finished"
    );
    result
}

#[cfg(test)]
#[path = "tests/writer_tests.rs"]
mod tests;
