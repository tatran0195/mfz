//! Bounded SSM reliability, separate from socket I/O.
//!
//! Selective ACKs release wire buffers, but not sequence-span credit beyond the
//! oldest gap. That bounds the AGENT's future-message buffer as well as ours.
use super::protocol::*;
use crate::error::{AppError, Result};
use bytes::Bytes;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio::time::Instant;
use uuid::Uuid;

pub(crate) const DEFAULT_WINDOW_MESSAGES: usize = 8192;
pub(crate) const MAX_INCOMING_FRAMES: usize = 8192;
pub(crate) const MAX_INCOMING_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const INITIAL_RTO: Duration = Duration::from_secs(1);
pub(crate) const MIN_RTO: Duration = Duration::from_millis(100);
pub(crate) const MAX_RTO: Duration = Duration::from_secs(5);
pub(crate) const MAX_RETRY_DELAY: Duration = Duration::from_secs(10);
pub(crate) const MESSAGE_LIFETIME: Duration = Duration::from_secs(300);
pub(crate) const MAX_RETRIES: u32 = 64;
pub(crate) const MAX_RETRY_PROBES: usize = 8;

/// Memory/latency limits, not an artificial bytes-per-second throttle.
#[derive(Debug, Clone, Copy)]
pub struct TunnelOptions {
    /// Maximum outstanding sequence span. Default 8192 (~8 MiB data), max 65536.
    pub window_messages: usize,
    /// Maximum new data frames per immediate flush. No wait to fill a batch.
    pub write_batch_messages: usize,
}
impl Default for TunnelOptions {
    fn default() -> Self {
        Self {
            window_messages: DEFAULT_WINDOW_MESSAGES,
            write_batch_messages: 64,
        }
    }
}
impl TunnelOptions {
    pub fn validate(&self) -> Result<()> {
        if !(1..=65536).contains(&self.window_messages)
            || !(1..=256).contains(&self.write_batch_messages)
        {
            return Err(AppError::Config(
                "SSM window must be 1..=65536 messages and write batch 1..=256".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) struct InputPacket {
    pub(crate) payload_type: u32,
    pub(crate) payload: Bytes,
}

/// RFC 6298-style EWMA, like the plugin's alpha=1/8 and beta=1/4 estimator.
/// Karn's rule is enforced by the caller: retransmitted packets are not samples.
#[derive(Debug)]
struct RttEstimator {
    srtt: Option<f64>,
    variation: f64,
    rto: Duration,
}
impl Default for RttEstimator {
    fn default() -> Self {
        Self {
            srtt: None,
            variation: 0.0,
            rto: INITIAL_RTO,
        }
    }
}
impl RttEstimator {
    fn sample(&mut self, sample: Duration) {
        let sample = sample.as_secs_f64();
        match self.srtt {
            None => {
                self.srtt = Some(sample);
                self.variation = sample / 2.0;
            }
            Some(previous) => {
                self.variation = 0.75 * self.variation + 0.25 * (previous - sample).abs();
                self.srtt = Some(0.875 * previous + 0.125 * sample);
            }
        }
        let seconds = self.srtt.unwrap_or(sample) + (4.0 * self.variation).max(0.010);
        // Round up to the plugin's 10 ms clock granularity, not down into loss.
        self.rto =
            Duration::from_secs_f64((seconds * 100.0).ceil() / 100.0).clamp(MIN_RTO, MAX_RTO);
    }
}

pub(crate) struct PendingMessage {
    wire: Bytes,
    id: Uuid,
    first_sent: Instant,
    last_sent: Instant,
    deadline: Instant,
    retries: u32,
}

pub(crate) struct OutgoingWindow {
    pub(crate) next_sequence: i64,
    pub(crate) pending: BTreeMap<i64, PendingMessage>,
    deadlines: BTreeMap<(Instant, i64), ()>,
    probes: BTreeSet<i64>,
    bytes: usize,
    options: TunnelOptions,
    rtt: RttEstimator,
    pub(crate) peak_messages: usize,
    pub(crate) peak_bytes: usize,
    pub(crate) retransmissions: u64,
}
impl Default for OutgoingWindow {
    fn default() -> Self {
        Self::new(TunnelOptions::default())
    }
}
impl OutgoingWindow {
    pub(crate) fn new(options: TunnelOptions) -> Self {
        Self {
            next_sequence: 0,
            pending: BTreeMap::new(),
            deadlines: BTreeMap::new(),
            probes: BTreeSet::new(),
            bytes: 0,
            options,
            rtt: RttEstimator::default(),
            peak_messages: 0,
            peak_bytes: 0,
            retransmissions: 0,
        }
    }
    pub(crate) fn has_capacity(&self) -> bool {
        let oldest = self
            .pending
            .first_key_value()
            .map_or(self.next_sequence, |(&sequence, _)| sequence);
        self.next_sequence - oldest < self.options.window_messages as i64
    }
    pub(crate) fn prepare(&mut self, packet: InputPacket, now: Instant) -> Result<Bytes> {
        if (packet.payload_type == PAYLOAD_TYPE_DATA
            && packet.payload.len() > MAX_STREAM_DATA_PAYLOAD)
            || packet.payload.len() > MAX_INCOMING_PAYLOAD
        {
            return Err(AppError::Ssm("SSM input exceeds its payload limit".into()));
        }
        if !self.has_capacity() {
            return Err(AppError::Ssm("SSM input sequence window is full".into()));
        }
        let frame = SsmFrame::new(
            msg_type("input_stream_data"),
            self.next_sequence,
            FLAG_NONE,
            packet.payload_type,
            packet.payload,
        );
        let wire = frame.encode();
        let byte_limit = self.options.window_messages * (HEADER_SIZE + MAX_STREAM_DATA_PAYLOAD);
        if self.bytes + wire.len() > byte_limit {
            return Err(AppError::Ssm(
                "SSM retained-wire byte budget is full".into(),
            ));
        }
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| AppError::Ssm("SSM sequence numbers exhausted".into()))?;
        let deadline = now + self.rtt.rto;
        self.bytes += wire.len();
        self.pending.insert(
            sequence,
            PendingMessage {
                wire: wire.clone(),
                id: frame.message_uuid(),
                first_sent: now,
                last_sent: now,
                deadline,
                retries: 0,
            },
        );
        self.deadlines.insert((deadline, sequence), ());
        self.peak_messages = self.peak_messages.max(self.pending.len());
        self.peak_bytes = self.peak_bytes.max(self.bytes);
        Ok(wire)
    }
    pub(crate) fn acknowledge(&mut self, ack: &AckPayload, now: Instant) -> bool {
        if !ack.is_sequential_message
            || ack.acknowledged_message_type != AckMessageType::InputStreamData
        {
            return false;
        }
        let sequence = ack.acknowledged_message_sequence_number;
        if !self
            .pending
            .get(&sequence)
            .is_some_and(|pending| pending.id == ack.acknowledged_message_id)
        {
            return false;
        }
        let pending = self
            .pending
            .remove(&sequence)
            .expect("matched ACK retains the same entry");
        self.deadlines.remove(&(pending.deadline, sequence));
        self.probes.remove(&sequence);
        self.bytes -= pending.wire.len();
        if pending.retries == 0 {
            self.rtt
                .sample(now.saturating_duration_since(pending.last_sent));
        }
        // Update only the oldest gap, not every pending record on every ACK.
        // This avoids an O(window) RTT-update hot path and speeds ordered recovery.
        self.refresh_oldest();
        true
    }
    fn refresh_oldest(&mut self) {
        if let Some((&sequence, pending)) = self.pending.first_key_value() {
            if pending.retries != 0 {
                return;
            }
            let old = pending.deadline;
            let new = (pending.last_sent + self.rtt.rto).min(pending.first_sent + MESSAGE_LIFETIME);
            if new == old {
                return;
            }
            self.deadlines.remove(&(old, sequence));
            self.deadlines.insert((new, sequence), ());
            self.pending
                .get_mut(&sequence)
                .expect("oldest entry exists")
                .deadline = new;
        }
    }
    /// Start the RTT/retry clock after the batch actually flushes, not on enqueue.
    pub(crate) fn mark_flushed(&mut self, sequences: &[i64], now: Instant) {
        for sequence in sequences {
            let Some(pending) = self.pending.get_mut(sequence) else {
                continue;
            };
            self.deadlines.remove(&(pending.deadline, *sequence));
            pending.last_sent = now;
            let delay =
                (self.rtt.rto * 2u32.saturating_pow(pending.retries.min(16))).min(MAX_RETRY_DELAY);
            pending.deadline = (now + delay).min(pending.first_sent + MESSAGE_LIFETIME);
            self.deadlines.insert((pending.deadline, *sequence), ());
        }
    }
    fn next_eligible_retry(&self) -> Option<(Instant, i64)> {
        if self.probes.len() < MAX_RETRY_PROBES {
            self.deadlines.first_key_value().map(|(&key, _)| key)
        } else {
            // Do not replay an entire large window when an agent stops ACKing.
            // Probe a bounded set of gaps; ACK/backoff makes more probes eligible.
            // This is at most eight entries, not an O(window) scan.
            self.probes
                .iter()
                .filter_map(|sequence| {
                    self.pending
                        .get(sequence)
                        .map(|pending| (pending.deadline, *sequence))
                })
                .min()
        }
    }
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.next_eligible_retry().map(|(time, _)| time)
    }
    pub(crate) fn retry_due(&mut self, now: Instant) -> Result<Option<(i64, Bytes)>> {
        let Some((deadline, sequence)) = self.next_eligible_retry() else {
            return Ok(None);
        };
        if now < deadline {
            return Ok(None);
        }
        let pending = self
            .pending
            .get_mut(&sequence)
            .expect("deadline index and pending records agree");
        if now.saturating_duration_since(pending.first_sent) >= MESSAGE_LIFETIME
            || pending.retries >= MAX_RETRIES
        {
            return Err(AppError::Ssm(format!(
                "SSM input seq={sequence} made no acknowledgement progress within its retry budget"
            )));
        }
        self.deadlines.remove(&(deadline, sequence));
        self.probes.insert(sequence);
        pending.retries += 1;
        pending.last_sent = now;
        let delay =
            (self.rtt.rto * 2u32.saturating_pow(pending.retries.min(16))).min(MAX_RETRY_DELAY);
        pending.deadline = (now + delay).min(pending.first_sent + MESSAGE_LIFETIME);
        self.deadlines.insert((pending.deadline, sequence), ());
        self.retransmissions += 1;
        // Reference-counted immutable bytes: UUID/sequence/date/digest stay exact.
        Ok(Some((sequence, pending.wire.clone())))
    }
    pub(crate) fn rto(&self) -> Duration {
        self.rtt.rto
    }
}

pub(crate) struct IncomingWindow {
    pub(crate) expected: i64,
    pub(crate) frames: BTreeMap<i64, SsmFrame>,
    pub(crate) bytes: usize,
}
impl IncomingWindow {
    pub(crate) fn new(expected: i64) -> Self {
        Self {
            expected,
            frames: BTreeMap::new(),
            bytes: 0,
        }
    }
    /// ACK only bytes we own or have already delivered. Reserve room for a gap.
    pub(crate) fn admit(&mut self, mut frame: SsmFrame) -> Result<bool> {
        if frame.sequence_number < 0
            || frame.sequence_number == i64::MAX
            || frame.payload.len() > MAX_INCOMING_PAYLOAD
        {
            return Err(AppError::Ssm(
                "Invalid SSM output sequence or payload size".into(),
            ));
        }
        if frame.sequence_number < self.expected {
            return Ok(true);
        }
        if let Some(previous) = self.frames.get(&frame.sequence_number) {
            if previous.message_id != frame.message_id
                || previous.payload != frame.payload
                || previous.flags != frame.flags
                || previous.payload_type != frame.payload_type
            {
                return Err(AppError::Ssm(
                    "Conflicting SSM output sequence replay".into(),
                ));
            }
            return Ok(true);
        }
        let (count_limit, byte_limit) = if frame.sequence_number == self.expected {
            (MAX_INCOMING_FRAMES, MAX_INCOMING_BYTES)
        } else {
            (
                MAX_INCOMING_FRAMES - 1,
                MAX_INCOMING_BYTES - MAX_INCOMING_PAYLOAD - HEADER_SIZE,
            )
        };
        // Charge the header too: tiny frames cannot bypass the memory/count limit.
        let charge = HEADER_SIZE + frame.payload.len();
        if self.frames.len() >= count_limit || self.bytes + charge > byte_limit {
            return Ok(false);
        }
        // Normalize before retaining: WebSocket Bytes slices can pin 128 KiB
        // backing allocations for a one-byte payload and defeat byte accounting.
        frame.payload = crate::transport_buffer::retain_bytes(frame.payload);
        self.bytes += charge;
        self.frames.insert(frame.sequence_number, frame);
        Ok(true)
    }
    pub(crate) fn has_ready(&self) -> bool {
        self.frames.contains_key(&self.expected)
    }
    pub(crate) fn pop_ready(&mut self) -> Option<SsmFrame> {
        let frame = self.frames.remove(&self.expected)?;
        self.bytes -= HEADER_SIZE + frame.payload.len();
        self.expected += 1;
        Some(frame)
    }
}

#[cfg(test)]
#[path = "tests/flow_tests.rs"]
mod tests;
