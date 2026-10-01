//! Lightweight, robust smux v1 protocol implementation for AWS SSM port forwarding.
//!
//! Compatible with xtaci/smux v1 as used by AWS Session Manager Plugin and SSM Agent.

use std::{
    collections::HashMap,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};

use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

pub const VERSION: u8 = 1;
pub const CMD_SYN: u8 = 0;
pub const CMD_FIN: u8 = 1;
pub const CMD_PSH: u8 = 2;
pub const CMD_NOP: u8 = 3;

pub const HEADER_SIZE: usize = 8;
pub const MAX_FRAME_PAYLOAD: usize = 32 * 1024;
pub const STREAM_QUEUE_DEPTH: usize = 256;

/// Encodes an smux v1 frame with little-endian length and stream_id.
pub fn encode(cmd: u8, stream_id: u32, payload: &[u8]) -> Bytes {
    debug_assert!(payload.len() <= MAX_FRAME_PAYLOAD);
    let mut buf = BytesMut::with_capacity(HEADER_SIZE + payload.len());
    buf.put_u8(VERSION);
    buf.put_u8(cmd);
    buf.put_u16_le(payload.len() as u16);
    buf.put_u32_le(stream_id);
    buf.put_slice(payload);
    buf.freeze()
}

/// Encodes a 0-length control frame (SYN, FIN, NOP).
#[inline]
pub fn control(cmd: u8, stream_id: u32) -> Bytes {
    encode(cmd, stream_id, &[])
}

pub struct DecodedFrame {
    pub cmd: u8,
    pub stream_id: u32,
    pub payload: Bytes,
}

pub enum Decoded {
    Frame(DecodedFrame),
    Incomplete,
    Corrupt(&'static str),
}

/// Decodes one complete frame from `buf`, advancing the buffer on success.
pub fn decode_frame(buf: &mut BytesMut) -> Decoded {
    if buf.len() < HEADER_SIZE {
        return Decoded::Incomplete;
    }
    if buf[0] != VERSION {
        warn!(
            "SSM smux: decode failed: byte 0 is 0x{:02x} (expected 0x{:02x}), buf_len={}, hex={:02x?}",
            buf[0],
            VERSION,
            buf.len(),
            &buf[..buf.len().min(32)]
        );
        return Decoded::Corrupt("unexpected smux version byte");
    }
    let length = u16::from_le_bytes([buf[2], buf[3]]) as usize;
    if length > MAX_FRAME_PAYLOAD {
        return Decoded::Corrupt("smux frame length exceeds maximum allowed");
    }
    if buf.len() < HEADER_SIZE + length {
        return Decoded::Incomplete;
    }

    buf.advance(1); // skip version
    let cmd = buf.get_u8();
    buf.advance(2); // skip length bytes
    let stream_id = buf.get_u32_le();
    let payload = buf.split_to(length).freeze();

    Decoded::Frame(DecodedFrame {
        cmd,
        stream_id,
        payload,
    })
}

#[derive(Debug)]
struct StreamEntry {
    inbound: mpsc::Sender<Bytes>,
}

#[derive(Debug)]
struct Inner {
    streams: Mutex<HashMap<u32, StreamEntry>>,
    frames: mpsc::UnboundedSender<Bytes>,
    next_id: AtomicU32,
    closed: AtomicBool,
}

impl Inner {
    fn route_push(&self, stream_id: u32, payload: Bytes) {
        let inbound = {
            let streams = self.streams.lock().unwrap();
            streams.get(&stream_id).map(|e| e.inbound.clone())
        };

        let Some(inbound) = inbound else {
            trace!("SSM smux: data for unknown stream {stream_id}; replying with FIN");
            let _ = self.frames.send(control(CMD_FIN, stream_id));
            return;
        };

        match inbound.try_send(payload) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("SSM smux: stream {stream_id} buffer full; evicting");
                self.streams.lock().unwrap().remove(&stream_id);
                let _ = self.frames.send(control(CMD_FIN, stream_id));
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.streams.lock().unwrap().remove(&stream_id);
            }
        }
    }

    fn route_fin(&self, stream_id: u32) {
        let mut streams = self.streams.lock().unwrap();
        streams.remove(&stream_id);
    }
}

/// A client-side smux session for multiplexing connections over SSM.
#[derive(Clone, Debug)]
pub struct SmuxSession {
    inner: Arc<Inner>,
}

impl SmuxSession {
    /// Creates a new smux session. Returns the session and the receiver for outbound smux frames
    /// that must be wrapped in `input_stream_data` and sent over WebSocket.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<Bytes>) {
        let (frames, frame_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            streams: Mutex::new(HashMap::new()),
            frames,
            // Client streams use odd IDs (1, 3, 5...)
            next_id: AtomicU32::new(1),
            closed: AtomicBool::new(false),
        });
        (Self { inner }, frame_rx)
    }

    /// Dispatches inbound raw bytes received from `output_stream_data` payloads.
    pub fn dispatch(&self, buf: &mut BytesMut) -> Result<(), &'static str> {
        loop {
            match decode_frame(buf) {
                Decoded::Incomplete => return Ok(()),
                Decoded::Corrupt(err) => {
                    warn!(
                        "SSM smux: corrupt frame: {err}, buf_len={}, hex={:02x?}",
                        buf.len(),
                        &buf[..buf.len().min(32)]
                    );
                    buf.clear();
                    return Err(err);
                }
                Decoded::Frame(frame) => match frame.cmd {
                    CMD_PSH => self.inner.route_push(frame.stream_id, frame.payload),
                    CMD_FIN => {
                        trace!(
                            "SSM smux: stream {} closed by remote (FIN)",
                            frame.stream_id
                        );
                        self.inner.route_fin(frame.stream_id);
                    }
                    CMD_SYN | CMD_NOP => {
                        // Remote SYN or keepalive NOP is ignored
                    }
                    other => {
                        trace!("SSM smux: ignoring unknown cmd {other}");
                    }
                },
            }
        }
    }

    /// Opens a new logical smux stream (initiates with SYN frame).
    pub fn open_stream(&self) -> io::Result<SmuxStream> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "smux session is closed",
            ));
        }

        let stream_id = self.inner.next_id.fetch_add(2, Ordering::Relaxed);
        let (inbound, data_rx) = mpsc::channel(STREAM_QUEUE_DEPTH);

        self.inner
            .streams
            .lock()
            .unwrap()
            .insert(stream_id, StreamEntry { inbound });

        if let Err(e) = self.inner.frames.send(control(CMD_SYN, stream_id)) {
            self.inner.streams.lock().unwrap().remove(&stream_id);
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("failed to send smux SYN: {e}"),
            ));
        }

        debug!("SSM smux: opened stream {stream_id}");
        Ok(SmuxStream {
            stream_id,
            inner: Arc::clone(&self.inner),
            data_rx,
            partial: None,
            read_closed: false,
            write_closed: false,
            frames: self.inner.frames.clone(),
        })
    }

    /// Closes all active streams and marks session as closed.
    pub fn close(&self) {
        if self.inner.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut streams = self.inner.streams.lock().unwrap();
        streams.clear();
    }
}

/// An individual logical stream inside the smux session.
/// Implements `AsyncRead` and `AsyncWrite`.
#[derive(Debug)]
pub struct SmuxStream {
    stream_id: u32,
    inner: Arc<Inner>,
    data_rx: mpsc::Receiver<Bytes>,
    partial: Option<Bytes>,
    read_closed: bool,
    write_closed: bool,
    frames: mpsc::UnboundedSender<Bytes>,
}

impl SmuxStream {
    pub fn id(&self) -> u32 {
        self.stream_id
    }
}

impl AsyncRead for SmuxStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();

        if this.read_closed {
            return Poll::Ready(Ok(()));
        }

        if let Some(chunk) = this.partial.as_mut() {
            let n = chunk.len().min(buf.remaining());
            buf.put_slice(&chunk[..n]);
            chunk.advance(n);
            if chunk.is_empty() {
                this.partial = None;
            }
            return Poll::Ready(Ok(()));
        }

        match this.data_rx.poll_recv(cx) {
            Poll::Ready(Some(mut chunk)) => {
                let n = chunk.len().min(buf.remaining());
                buf.put_slice(&chunk[..n]);
                chunk.advance(n);
                if !chunk.is_empty() {
                    this.partial = Some(chunk);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => {
                this.read_closed = true;
                Poll::Ready(Ok(()))
            }
            Poll::Pending => {
                if this.inner.closed.load(Ordering::Acquire) {
                    this.read_closed = true;
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending
            }
        }
    }
}

impl AsyncWrite for SmuxStream {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        if this.write_closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "smux stream is write-closed",
            )));
        }
        if this.inner.closed.load(Ordering::Acquire) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "smux session is closed",
            )));
        }

        let n = buf.len().min(MAX_FRAME_PAYLOAD);
        trace!("SSM smux: TCP sent stream {} len={}", this.stream_id, n);
        this.frames
            .send(encode(CMD_PSH, this.stream_id, &buf[..n]))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "smux session is closed"))?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.write_closed {
            return Poll::Ready(Ok(()));
        }
        this.write_closed = true;
        let _ = this.frames.send(control(CMD_FIN, this.stream_id));
        trace!("SSM smux: sent FIN for stream {}", this.stream_id);
        Poll::Ready(Ok(()))
    }
}

impl Drop for SmuxStream {
    fn drop(&mut self) {
        if !self.write_closed {
            let _ = self.inner.frames.send(control(CMD_FIN, self.stream_id));
        }
        let mut streams = self.inner.streams.lock().unwrap();
        streams.remove(&self.stream_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout_matches_smux_v1() {
        let frame = encode(CMD_PSH, 3, b"hello");
        assert_eq!(frame.len(), HEADER_SIZE + 5);
        assert_eq!(frame[0], VERSION);
        assert_eq!(frame[1], CMD_PSH);
        assert_eq!(&frame[2..4], &[5, 0], "length is little-endian");
        assert_eq!(&frame[4..8], &[3, 0, 0, 0], "stream id is little-endian");
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
        assert_eq!(decoded[0].0, CMD_PSH);
        assert_eq!(&decoded[0].2[..], b"first");
        assert_eq!(decoded[1].0, CMD_FIN);
        assert_eq!(decoded[2].1, 3);
        assert_eq!(&decoded[2].2[..], b"second");
    }
}
