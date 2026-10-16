//! smux v1 client, compatible with AWS's vendored xtaci/smux.
//!
//! A SESSION byte budget (not a per-stream await) bounds receive storage without
//! head-of-line blocking on a single slow reader until the shared budget is full.
//! Per-stream bounded send queues and round-robin scheduling stop a bulk sender
//! monopolising the FIFO. FIN is scheduled after queued data, even during Drop.
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures_util::{future::poll_fn, task::AtomicWaker};
use std::{
    collections::{HashMap, VecDeque},
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{mpsc::error::TryRecvError, OwnedSemaphorePermit, Semaphore},
};
use tokio_util::sync::CancellationToken;

pub const VERSION: u8 = 1;
pub const CMD_SYN: u8 = 0;
pub const CMD_FIN: u8 = 1;
pub const CMD_PSH: u8 = 2;
pub const CMD_NOP: u8 = 3;
pub const HEADER_SIZE: usize = 8;
pub const MAX_FRAME_PAYLOAD: usize = 32 * 1024; // AWS's send default, not the u16 wire maximum.
pub const STREAM_WRITE_QUEUE_DEPTH: usize = 2;
pub const MAX_STREAMS: usize = 64;
pub const RECEIVE_BUFFER_BYTES: usize = 4 * 1024 * 1024; // xtaci/smux default session budget.
const CHUNK_OVERHEAD: usize = 128;

pub fn encode(cmd: u8, stream_id: u32, payload: &[u8]) -> Bytes {
    assert!(
        payload.len() <= u16::MAX as usize,
        "smux payload must fit its u16 wire length"
    );
    let mut wire = BytesMut::with_capacity(HEADER_SIZE + payload.len());
    wire.put_u8(VERSION);
    wire.put_u8(cmd);
    wire.put_u16_le(payload.len() as u16);
    wire.put_u32_le(stream_id);
    wire.put_slice(payload);
    wire.freeze()
}
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
pub fn decode_frame(buffer: &mut BytesMut) -> Decoded {
    if buffer.len() < HEADER_SIZE {
        return Decoded::Incomplete;
    }
    if buffer[0] != VERSION {
        return Decoded::Corrupt("unexpected smux version");
    }
    let cmd = buffer[1];
    if !matches!(cmd, CMD_SYN | CMD_FIN | CMD_PSH | CMD_NOP) {
        return Decoded::Corrupt("unknown smux v1 command");
    }
    let length = u16::from_le_bytes([buffer[2], buffer[3]]) as usize;
    if cmd != CMD_PSH && length != 0 {
        return Decoded::Corrupt("control frame has a payload");
    }
    if buffer.len() < HEADER_SIZE + length {
        return Decoded::Incomplete;
    }
    let stream_id = u32::from_le_bytes(buffer[4..8].try_into().expect("complete header"));
    buffer.advance(HEADER_SIZE);
    let backing_capacity = buffer.capacity();
    let mut payload = buffer.split_to(length).freeze();
    // A tiny slice must not pin an arbitrarily large reassembly allocation.
    if length > 0 && backing_capacity > (length + CHUNK_OVERHEAD) * 4 {
        payload = Bytes::copy_from_slice(&payload);
    }
    Decoded::Frame(DecodedFrame {
        cmd,
        stream_id,
        payload,
    })
}

#[derive(Debug)]
struct Chunk {
    bytes: Bytes,
    _credit: OwnedSemaphorePermit,
}
#[derive(Debug, Default)]
struct ReceiveQueue {
    chunks: VecDeque<Chunk>,
    fin: bool,
}
#[derive(Debug)]
struct StreamState {
    receive: Mutex<ReceiveQueue>,
    read_waker: AtomicWaker,
    write_waker: AtomicWaker,
    local_closed: AtomicBool,
    stopped: CancellationToken,
}
impl StreamState {
    fn new() -> Self {
        Self {
            receive: Mutex::new(ReceiveQueue::default()),
            read_waker: AtomicWaker::new(),
            write_waker: AtomicWaker::new(),
            local_closed: AtomicBool::new(false),
            stopped: CancellationToken::new(),
        }
    }
    fn close(&self) {
        self.local_closed.store(true, Ordering::Release);
        self.stopped.cancel();
        self.receive
            .lock()
            .expect("receive queue lock")
            .chunks
            .clear();
        self.read_waker.wake();
        self.write_waker.wake();
    }
}
#[derive(Debug)]
struct SendEntry {
    state: Arc<StreamState>,
    frames: VecDeque<Bytes>,
    scheduled: bool,
    fin_requested: bool,
    fin_sent: bool,
}
#[derive(Debug, Default)]
struct Scheduler {
    streams: HashMap<u32, SendEntry>,
    ready: VecDeque<u32>,
    keepalive: bool,
}
#[derive(Debug)]
struct Inner {
    scheduler: Mutex<Scheduler>,
    outbound_waker: AtomicWaker,
    next_id: AtomicU32,
    closed: AtomicBool,
    shutdown: CancellationToken,
    receive_credit: Arc<Semaphore>,
}
fn closed_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "smux session or stream is closed",
    )
}
impl Inner {
    fn request_fin(&self, id: u32) {
        let mut scheduler = self.scheduler.lock().expect("send scheduler lock");
        let schedule = if let Some(entry) = scheduler.streams.get_mut(&id) {
            entry.fin_requested = true;
            if !entry.scheduled && !entry.fin_sent {
                entry.scheduled = true;
                true
            } else {
                false
            }
        } else {
            false
        };
        if schedule {
            scheduler.ready.push_back(id);
        }
        drop(scheduler);
        self.outbound_waker.wake();
    }
    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.shutdown.cancel();
        self.receive_credit.close();
        let states = {
            let mut scheduler = self.scheduler.lock().expect("send scheduler lock");
            let states = scheduler
                .streams
                .values()
                .map(|entry| entry.state.clone())
                .collect::<Vec<_>>();
            scheduler.streams.clear();
            scheduler.ready.clear();
            scheduler.keepalive = false;
            states
        };
        for state in states {
            state.close();
        }
        self.outbound_waker.wake();
    }
    async fn route_push(&self, id: u32, bytes: Bytes) -> io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let state = self
            .scheduler
            .lock()
            .expect("send scheduler lock")
            .streams
            .get(&id)
            .map(|entry| entry.state.clone());
        let Some(state) = state else {
            return Ok(());
        };
        if state.local_closed.load(Ordering::Acquire)
            || state.receive.lock().expect("receive queue lock").fin
        {
            return Ok(());
        }
        let credit = tokio::select! {
            biased;
            _=state.stopped.cancelled()=>return Ok(()),
            result=self.receive_credit.clone().acquire_many_owned((bytes.len()+CHUNK_OVERHEAD) as u32)=>{
                match result {Ok(credit)=>credit,Err(_)=>return Ok(())}
            }
        };
        {
            let mut receive = state.receive.lock().expect("receive queue lock");
            if state.local_closed.load(Ordering::Acquire) || receive.fin {
                return Ok(());
            }
            let bytes = crate::transport::buffer::retain_bytes(bytes);
            receive.chunks.push_back(Chunk {
                bytes,
                _credit: credit,
            });
        }
        state.read_waker.wake();
        Ok(())
    }
    fn route_fin(&self, id: u32) {
        let state = self
            .scheduler
            .lock()
            .expect("send scheduler lock")
            .streams
            .get(&id)
            .map(|entry| entry.state.clone());
        if let Some(state) = state {
            state.receive.lock().expect("receive queue lock").fin = true;
            state.read_waker.wake();
        }
    }
}

#[derive(Clone, Debug)]
pub struct SmuxSession {
    inner: Arc<Inner>,
}
impl SmuxSession {
    pub fn new() -> (Self, SmuxOutbound) {
        let inner = Arc::new(Inner {
            scheduler: Mutex::new(Scheduler::default()),
            outbound_waker: AtomicWaker::new(),
            next_id: AtomicU32::new(1),
            closed: AtomicBool::new(false),
            shutdown: CancellationToken::new(),
            receive_credit: Arc::new(Semaphore::new(RECEIVE_BUFFER_BYTES)),
        });
        (
            Self {
                inner: inner.clone(),
            },
            SmuxOutbound { inner },
        )
    }
    pub async fn open_stream(&self) -> io::Result<SmuxStream> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(closed_error());
        }
        #[allow(deprecated)] // fetch_update is also available on our Rust MSRV.
        let id = self
            .inner
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(2))
            .map_err(|_| io::Error::other("smux stream IDs exhausted"))?;
        let state = Arc::new(StreamState::new());
        {
            let mut scheduler = self.inner.scheduler.lock().expect("send scheduler lock");
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(closed_error());
            }
            if scheduler.streams.len() >= MAX_STREAMS {
                return Err(io::Error::other("smux active/closing stream limit reached"));
            }
            scheduler.streams.insert(
                id,
                SendEntry {
                    state: state.clone(),
                    frames: VecDeque::from([control(CMD_SYN, id)]),
                    scheduled: true,
                    fin_requested: false,
                    fin_sent: false,
                },
            );
            scheduler.ready.push_back(id);
        }
        self.inner.outbound_waker.wake();
        // No await between registration and ownership transfer: cancellation cannot
        // strand a registered stream or SYN waiting on an unrelated global FIFO.
        Ok(SmuxStream {
            id,
            inner: self.inner.clone(),
            state,
            write_closed: false,
        })
    }
    pub async fn dispatch(&self, buffer: &mut BytesMut) -> io::Result<()> {
        loop {
            match decode_frame(buffer) {
                Decoded::Incomplete => return Ok(()),
                Decoded::Corrupt(reason) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, reason))
                }
                Decoded::Frame(frame) => match frame.cmd {
                    CMD_PSH => {
                        self.inner
                            .route_push(frame.stream_id, frame.payload)
                            .await?
                    }
                    CMD_FIN => self.inner.route_fin(frame.stream_id),
                    CMD_SYN | CMD_NOP => {}
                    _ => unreachable!("decoder validates command"),
                },
            }
        }
    }
    pub fn close(&self) {
        self.inner.close();
    }
    pub async fn closed(&self) {
        self.inner.shutdown.cancelled().await;
    }
    pub async fn send_keepalive(&self) -> io::Result<()> {
        let mut scheduler = self.inner.scheduler.lock().expect("send scheduler lock");
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(closed_error());
        }
        // At most one NOP is queued, regardless of how long output is stalled.
        if !scheduler.keepalive {
            scheduler.keepalive = true;
            scheduler.ready.push_back(0);
        }
        drop(scheduler);
        self.inner.outbound_waker.wake();
        Ok(())
    }
}

/// Round-robin frame source; no stale-ID channel or unbounded tombstone queue.
#[derive(Debug)]
pub struct SmuxOutbound {
    inner: Arc<Inner>,
}
impl SmuxOutbound {
    fn pop(&self) -> Option<Bytes> {
        let mut scheduler = self.inner.scheduler.lock().expect("send scheduler lock");
        while let Some(id) = scheduler.ready.pop_front() {
            if id == 0 {
                scheduler.keepalive = false;
                return Some(control(CMD_NOP, 0));
            }
            let Some(entry) = scheduler.streams.get_mut(&id) else {
                continue;
            };
            entry.scheduled = false;
            let wake = entry.state.clone();
            let frame = if let Some(frame) = entry.frames.pop_front() {
                Some(frame)
            } else if entry.fin_requested && !entry.fin_sent {
                entry.fin_sent = true;
                Some(control(CMD_FIN, id))
            } else {
                None
            };
            let again = !entry.frames.is_empty() || (entry.fin_requested && !entry.fin_sent);
            let remove = entry.fin_sent && entry.state.local_closed.load(Ordering::Acquire);
            if again {
                entry.scheduled = true;
                scheduler.ready.push_back(id);
            }
            if remove {
                scheduler.streams.remove(&id);
            }
            if let Some(frame) = frame {
                drop(scheduler);
                wake.write_waker.wake();
                return Some(frame);
            }
        }
        None
    }
    pub fn try_recv(&mut self) -> Result<Bytes, TryRecvError> {
        if let Some(frame) = self.pop() {
            return Ok(frame);
        }
        if self.inner.closed.load(Ordering::Acquire) {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
    pub async fn recv(&mut self) -> Option<Bytes> {
        poll_fn(|cx| {
            self.inner.outbound_waker.register(cx.waker());
            if let Some(frame) = self.pop() {
                return Poll::Ready(Some(frame));
            }
            if self.inner.closed.load(Ordering::Acquire) {
                Poll::Ready(None)
            } else {
                Poll::Pending
            }
        })
        .await
    }
}
impl Drop for SmuxOutbound {
    fn drop(&mut self) {
        self.inner.close();
    }
}

#[derive(Debug)]
pub struct SmuxStream {
    id: u32,
    inner: Arc<Inner>,
    state: Arc<StreamState>,
    write_closed: bool,
}
impl SmuxStream {
    pub fn id(&self) -> u32 {
        self.id
    }
}
impl AsyncRead for SmuxStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut receive = this.state.receive.lock().expect("receive queue lock");
        if let Some(chunk) = receive.chunks.front_mut() {
            let n = buffer.remaining().min(chunk.bytes.len());
            buffer.put_slice(&chunk.bytes[..n]);
            chunk.bytes.advance(n);
            if chunk.bytes.is_empty() {
                receive.chunks.pop_front();
            } // return whole retained-allocation credit.
            return Poll::Ready(Ok(()));
        }
        if receive.fin
            || this.state.local_closed.load(Ordering::Acquire)
            || this.inner.closed.load(Ordering::Acquire)
        {
            return Poll::Ready(Ok(()));
        }
        // Register while holding the receive lock; enqueue cannot miss the wake.
        this.state.read_waker.register(cx.waker());
        Poll::Pending
    }
}
impl AsyncWrite for SmuxStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.write_closed || this.inner.closed.load(Ordering::Acquire) {
            return Poll::Ready(Err(closed_error()));
        }
        if bytes.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut scheduler = this.inner.scheduler.lock().expect("send scheduler lock");
        let Some(entry) = scheduler.streams.get_mut(&this.id) else {
            return Poll::Ready(Err(closed_error()));
        };
        if entry.fin_requested {
            return Poll::Ready(Err(closed_error()));
        }
        if entry.frames.len() >= STREAM_WRITE_QUEUE_DEPTH {
            entry.state.write_waker.register(cx.waker());
            return Poll::Pending;
        }
        let n = bytes.len().min(MAX_FRAME_PAYLOAD);
        entry
            .frames
            .push_back(encode(CMD_PSH, this.id, &bytes[..n]));
        let schedule = !entry.scheduled;
        if schedule {
            entry.scheduled = true;
            scheduler.ready.push_back(this.id);
        }
        drop(scheduler);
        this.inner.outbound_waker.wake();
        // No Pending after accepting bytes: individual write() remains cancel-safe.
        Poll::Ready(Ok(n))
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.inner.closed.load(Ordering::Acquire) {
            return Poll::Ready(Err(closed_error()));
        }
        let scheduler = this.inner.scheduler.lock().expect("send scheduler lock");
        let Some(entry) = scheduler.streams.get(&this.id) else {
            return Poll::Ready(Err(closed_error()));
        };
        if entry.frames.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            entry.state.write_waker.register(cx.waker());
            Poll::Pending
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.write_closed {
            this.write_closed = true;
            this.inner.request_fin(this.id);
        }
        Poll::Ready(Ok(()))
    }
}
impl Drop for SmuxStream {
    fn drop(&mut self) {
        self.state.close();
        self.inner.request_fin(self.id);
        let mut scheduler = self.inner.scheduler.lock().expect("send scheduler lock");
        if scheduler
            .streams
            .get(&self.id)
            .is_some_and(|entry| entry.fin_sent)
        {
            scheduler.streams.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests;
