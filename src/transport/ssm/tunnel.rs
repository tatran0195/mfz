//! Native Rust SSM Session Manager port-forwarding tunnel.
//!
//! Replaces the `session-manager-plugin` binary by implementing the SSM
//! streaming WebSocket protocol directly.
//!
//! # Protocol overview
//!
//! 1. Call `ssm:StartSession` (via `aws-sdk-ssm`) → get `stream_url` + `token_value`
//! 2. Open a WSS connection to `stream_url`
//! 3. Send the auth token as a JSON text frame
//! 4. Complete the SSM agent handshake (publication alone is not readiness)
//! 5. Bind a TCP listener on `local_port`; relay bidirectional data between
//!    the accepted TCP socket and the WebSocket using SSM binary frames
//!
//! # SSM binary frame layout (all fields big-endian)
//!
//! ```text
//! Offset  Size  Field
//! ──────  ────  ──────────────────────────────────
//!      0     4  HeaderLength  (116; payload starts at offset 120)
//!      4    32  MessageType   (ASCII, space-padded)
//!     36     4  SchemaVersion (always 1)
//!     40     8  CreatedDate   (Unix ms, u64)
//!     48     8  SequenceNumber (i64)
//!     56     8  Flags         (u64; 0=none, 1=SYN, 2=FIN)
//!     64    16  MessageId     (UUID v4 bytes)
//!     80    32  PayloadDigest (SHA-256 of payload)
//!    112     4  PayloadType
//!    116     4  PayloadLength
//!    120+    N  Payload
//! ```

use std::time::Duration;

use aws_sigv4::{
    http_request::{sign, SignableBody, SignableRequest, SigningSettings},
    sign::v4::SigningParams,
};
use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Error as WsError, Message},
    WebSocketStream,
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

pub use super::flow::TunnelOptions;
use super::{
    flow::{IncomingWindow, InputPacket},
    protocol::*,
    writer::{
        acknowledgement, control, send_ws, try_acknowledgement, try_control,
        websocket_writer_with_options, WriterControl, CONTROL_QUEUE_DEPTH, INPUT_QUEUE_DEPTH,
    },
};
use crate::error::{AppError, Result};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const DISPATCH_QUEUE_DEPTH: usize = 64;

#[cfg(test)]
use super::{flow::*, writer::websocket_writer};
#[cfg(test)]
use tokio::time::Instant;

// ── Public tunnel handle ───────────────────────────────────────────────────────

/// Returned by [`start_port_forward_tunnel`]; drop or call [`stop`] to clean up.
pub struct SsmTunnelHandle {
    pub(crate) shutdown_tx: Option<oneshot::Sender<()>>,
    pub(crate) bound_rx: Option<oneshot::Receiver<()>>,
    pub(crate) task: Option<tokio::task::JoinHandle<Result<()>>>,
    closed: CancellationToken,
    ready: bool,
    exit_error: Option<String>,
}

impl SsmTunnelHandle {
    pub fn is_running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished()) && !self.closed.is_cancelled()
    }
    fn request_shutdown(&mut self) {
        self.ready = false;
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
    /// Emergency synchronous stop. Normal app paths prefer shutdown().await.
    pub fn stop(&mut self) {
        self.request_shutdown();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
    pub(crate) fn closed_token(&self) -> CancellationToken {
        self.closed.clone()
    }
    /// Completes on error, normal exit or abort, including abort before first poll.
    pub async fn closed(&self) {
        self.closed.cancelled().await;
    }
    /// Readiness is an authenticated completed handshake plus an actual TCP bind,
    /// never a probe socket, task-liveness guess, fixed sleep or polling interval.
    pub async fn wait_until_ready(&mut self) -> Result<()> {
        if self.ready && self.is_running() {
            return Ok(());
        }
        let Some(receiver) = self.bound_rx.take() else {
            return Err(AppError::Ssm(
                self.exit_error
                    .clone()
                    .unwrap_or_else(|| "Native tunnel did not become ready".into()),
            ));
        };
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver).await {
            Ok(Ok(())) if self.is_running() => {
                self.ready = true;
                Ok(())
            }
            Ok(Ok(())) => self.join().await.and(Err(AppError::Ssm(
                "Native tunnel exited immediately after binding".into(),
            ))),
            Ok(Err(_)) => {
                let result = self.join().await;
                result.and(Err(AppError::Ssm(
                    "Native tunnel exited before binding".into(),
                )))
            }
            Err(_) => Err(AppError::Ssm("Native tunnel readiness timed out".into())),
        }
    }
    async fn join(&mut self) -> Result<()> {
        let Some(task) = self.task.as_mut() else {
            return match &self.exit_error {
                Some(error) => Err(AppError::Ssm(error.clone())),
                None => Ok(()),
            };
        };
        let result = match task.await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(AppError::Ssm(format!("Native tunnel task failed: {error}"))),
        };
        self.task.take();
        if let Err(error) = &result {
            self.exit_error = Some(error.to_string());
        }
        result
    }
    /// Signals shutdown, then waits for owned tasks and sockets to release.
    /// A bounded abort fallback handles a broken child rather than hanging exit.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.request_shutdown();
        // Borrow until completion: cancelling this Future must not detach the
        // parent while its TCP/WebSocket/JoinSet children are still alive.
        let Some(task) = self.task.as_mut() else {
            return Ok(());
        };
        let result = match tokio::time::timeout(Duration::from_secs(2), &mut *task).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) if error.is_cancelled() => Ok(()),
            Ok(Err(error)) => Err(AppError::Ssm(format!("Native tunnel task failed: {error}"))),
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err(AppError::Ssm("Native tunnel shutdown timed out".into()))
            }
        };
        self.task.take();
        if let Err(error) = &result {
            self.exit_error = Some(error.to_string());
        }
        result
    }
}

impl Drop for SsmTunnelHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Session credentials (passed from SSM caller) ──────────────────────────────

/// Temporary AWS credentials used to sign the WebSocket upgrade request.
#[derive(Clone)]
pub struct SessionCredentials {
    pub credentials: aws_credential_types::Credentials,
    pub region: String,
}

// ── Entry point ────────────────────────────────────────────────────────────────

/// Connects to the SSM data-channel WebSocket and starts a TCP listener on
/// `local_port` that forwards traffic to the remote `remote_port`.
///
/// The `creds` are used to sign the WebSocket HTTP upgrade request with SigV4
/// (service `ssmmessages`) — this is required by the SSM service; without it
/// the server closes the connection with code=1000 reason=Bye.
///
/// Returns a background-task handle immediately. `bound_rx` signals readiness
/// only after WebSocket authentication, the agent handshake and the TCP bind.
pub async fn start_port_forward_tunnel(
    stream_url: String,
    token_value: String,
    local_port: u16,
    remote_port: u16,
    creds: SessionCredentials,
) -> Result<SsmTunnelHandle> {
    start_port_forward_tunnel_with_options(
        stream_url,
        token_value,
        local_port,
        remote_port,
        creds,
        TunnelOptions::default(),
    )
    .await
}

/// Advanced memory/batching limits. Never increase the SSM 1024-byte wire payload.
pub async fn start_port_forward_tunnel_with_options(
    stream_url: String,
    token_value: String,
    local_port: u16,
    _remote_port: u16,
    creds: SessionCredentials,
    options: TunnelOptions,
) -> Result<SsmTunnelHandle> {
    options.validate()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (bound_tx, bound_rx) = oneshot::channel::<()>();

    let closed = CancellationToken::new();
    let completed = closed.clone().drop_guard();
    let task = tokio::spawn(async move {
        let _completed = completed;
        let result = run_tunnel(
            stream_url,
            token_value,
            local_port,
            shutdown_rx,
            bound_tx,
            creds,
            options,
        )
        .await;
        if let Err(error) = &result {
            error!("SSM native tunnel exited with error: {error}");
        }
        result
    });

    Ok(SsmTunnelHandle {
        shutdown_tx: Some(shutdown_tx),
        bound_rx: Some(bound_rx),
        task: Some(task),
        closed,
        ready: false,
        exit_error: None,
    })
}

// ── Internal tunnel implementation ────────────────────────────────────────────

async fn run_tunnel(
    stream_url: String,
    token_value: String,
    local_port: u16,
    mut shutdown_rx: oneshot::Receiver<()>,
    bound_tx: oneshot::Sender<()>,
    creds: SessionCredentials,
    options: TunnelOptions,
) -> Result<()> {
    info!(
        "SSM: connecting WebSocket → {}",
        &stream_url[..stream_url.find('?').unwrap_or(stream_url.len())]
    );
    let mut request = stream_url
        .as_str()
        .into_client_request()
        .map_err(|error| AppError::Ssm(format!("Invalid stream URL: {error}")))?;
    sign_request_sigv4(&mut request, &creds)
        .map_err(|error| AppError::Ssm(format!("SigV4 signing failed: {error}")))?;
    let (ws, _) = tokio::select! {
        _ = &mut shutdown_rx => return Ok(()),
        result = tokio::time::timeout(HANDSHAKE_TIMEOUT, tokio_tungstenite::connect_async_with_config(request, Some(websocket_config()), true)) => {
            result.map_err(|_| AppError::Ssm("SSM WebSocket connect timed out".into()))?
                .map_err(|error| AppError::Ssm(format!("WebSocket connect failed: {error}")))?
        }
    };
    relay_websocket_with_options(ws, token_value, local_port, shutdown_rx, bound_tx, options).await
}

fn websocket_config() -> tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
    let mut config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
    config.read_buffer_size = 32 * 1024;
    config.write_buffer_size = 128 * 1024;
    config.max_write_buffer_size = 1024 * 1024;
    config.max_message_size = Some(HEADER_SIZE + MAX_INCOMING_PAYLOAD);
    config.max_frame_size = Some(HEADER_SIZE + MAX_INCOMING_PAYLOAD);
    config
}

fn child_result(
    result: Option<std::result::Result<Result<()>, tokio::task::JoinError>>,
) -> Result<()> {
    match result {
        Some(Ok(result)) => result,
        Some(Err(error)) => Err(AppError::Ssm(format!("SSM transport task failed: {error}"))),
        None => Err(AppError::Ssm(
            "SSM transport tasks stopped unexpectedly".into(),
        )),
    }
}

struct HandshakeState {
    incoming: IncomingWindow,
    keepalive: bool,
}

/// Separated from AWS/TLS setup so tests exercise the production relay against
/// an in-memory WebSocket peer, including handshake, ACKs and concurrent copies.
#[cfg(test)]
async fn relay_websocket<T>(
    ws: WebSocketStream<T>,
    token_value: String,
    local_port: u16,
    shutdown_rx: oneshot::Receiver<()>,
    bound_tx: oneshot::Sender<()>,
) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    relay_websocket_with_options(
        ws,
        token_value,
        local_port,
        shutdown_rx,
        bound_tx,
        TunnelOptions::default(),
    )
    .await
}

async fn relay_websocket_with_options<T>(
    mut ws: WebSocketStream<T>,
    token_value: String,
    local_port: u16,
    mut shutdown_rx: oneshot::Receiver<()>,
    bound_tx: oneshot::Sender<()>,
    options: TunnelOptions,
) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let token_json = serde_json::to_string(&TokenFrame {
        message_schema_version: "1.0",
        request_id: Uuid::new_v4().to_string(),
        token_value,
        client_id: Uuid::new_v4().to_string(),
        client_version: "1.3.0.0",
    })
    .map_err(|error| AppError::Ssm(format!("Token serialization failed: {error}")))?;
    // Never log the token frame: it contains the live SSM session token.
    tokio::select! {
        _ = &mut shutdown_rx => return Ok(()),
        result = send_ws(&mut ws, Message::Text(token_json.into())) => result?,
    }
    let (ws_sink, mut ws_src) = ws.split();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE_DEPTH);
    let (input_tx, input_rx) = mpsc::channel(INPUT_QUEUE_DEPTH);
    // Scalar ACK inbox sized for the entire valid window plus retry slack.
    let (ack_tx, ack_rx) = mpsc::channel(options.window_messages + CONTROL_QUEUE_DEPTH);
    // JoinSet aborts ALL owned children if this future is dropped/aborted.
    // The previous detached tokio::spawn tasks outlived SsmTunnelHandle::stop.
    let mut tasks = JoinSet::new();
    tasks.spawn(websocket_writer_with_options(
        ws_sink,
        control_rx,
        input_rx,
        options,
        Some(ack_rx),
    ));
    let handshake = tokio::select! {
        _ = &mut shutdown_rx => return Ok(()),
        result = tokio::time::timeout(HANDSHAKE_TIMEOUT,
            perform_handshake(&mut ws_src, &control_tx, &input_tx)) => {
            result.map_err(|_| AppError::Ssm("SSM agent handshake timed out".into()))??
        }
        result = tasks.join_next() => return child_result(result),
    };
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, local_port))
        .await
        .map_err(|error| AppError::Ssm(format!("TCP bind :{local_port} failed: {error}")))?;
    info!(
        "SSM: port forwarder listening on {}",
        listener.local_addr()?
    );
    let _ = bound_tx.send(());

    let (smux_session, mut outbound_rx) = crate::transport::smux::SmuxSession::new();
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<SsmFrame>(DISPATCH_QUEUE_DEPTH);
    let outbound_session = smux_session.clone();
    tasks.spawn(async move {
        loop {
            let bytes = tokio::select! {
                _ = outbound_session.closed() => return Ok(()),
                bytes = outbound_rx.recv() => match bytes {
                    Some(bytes) => bytes,
                    None => return Ok(()),
                }
            };
            for offset in (0..bytes.len()).step_by(MAX_STREAM_DATA_PAYLOAD) {
                let end = (offset + MAX_STREAM_DATA_PAYLOAD).min(bytes.len());
                input_tx
                    .send(InputPacket {
                        payload_type: PAYLOAD_TYPE_DATA,
                        payload: bytes.slice(offset..end),
                    })
                    .await
                    .map_err(|_| AppError::Ssm("SSM input writer stopped".into()))?;
            }
        }
    });
    let inbound_session = smux_session.clone();
    tasks.spawn(async move {
        let mut buffer = BytesMut::new();
        while let Some(frame) = incoming_rx.recv().await {
            if frame.payload_type == PAYLOAD_TYPE_DATA {
                buffer.extend_from_slice(&frame.payload);
                inbound_session
                    .dispatch(&mut buffer)
                    .await
                    .map_err(|error| AppError::Ssm(format!("smux decode failed: {error}")))?;
            } else if frame.payload_type == 10 {
                let flag = frame
                    .payload
                    .get(..4)
                    .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
                    .unwrap_or(0);
                if matches!(flag, 1..=3) {
                    return Err(AppError::Ssm(format!(
                        "Remote SSM control flag {flag} \
                        (1=disconnect, 2=terminate, 3=destination connection failed)"
                    )));
                }
            }
            if frame.flags & FLAG_FIN != 0 {
                return Ok(());
            }
        }
        Ok(())
    });
    tasks.spawn(websocket_reader_with_acks(
        ws_src,
        control_tx,
        incoming_tx,
        handshake.incoming,
        Some(ack_tx),
    ));
    if handshake.keepalive {
        let session = smux_session.clone();
        tasks.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = session.closed() => return Ok(()),
                    _ = tick.tick() => {
                        session.send_keepalive().await?;
                    }
                }
            }
        });
    }
    let mut connections = JoinSet::new();
    let result = loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                info!("SSM tunnel: shutdown requested");
                break Ok(());
            }
            result = tasks.join_next() => break child_result(result),
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    warn!("SSM tunnel: connection task failed: {error}");
                }
            }
            accept = listener.accept(), if connections.len() < crate::transport::smux::MAX_STREAMS => {
                match accept {
                    Ok((tcp, peer)) => {
                        let session = smux_session.clone();
                        // Do not block accept/shutdown on a full SYN/data queue.
                        connections.spawn(async move {
                            match session.open_stream().await {
                                Ok(stream) => forward_connection(tcp, peer, stream).await,
                                Err(error) => warn!("SSM: cannot open stream for {peer}: {error}"),
                            }
                        });
                    }
                    Err(error) => break Err(AppError::Ssm(format!("TCP accept failed: {error}"))),
                }
            }
        }
    };
    smux_session.close();
    // Abort and join before releasing the listener/tunnel, including failed sends.
    connections.shutdown().await;
    tasks.shutdown().await;
    result
}

async fn perform_handshake<S>(
    ws_src: &mut S,
    control_tx: &mpsc::Sender<WriterControl>,
    input_tx: &mpsc::Sender<InputPacket>,
) -> Result<HandshakeState>
where
    S: Stream<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let mut incoming = IncomingWindow::new(0);
    let mut saw_request = false;
    let mut keepalive = false;
    while let Some(message) = ws_src.next().await {
        match message
            .map_err(|error| AppError::Ssm(format!("SSM handshake receive failed: {error}")))?
        {
            Message::Binary(bytes) => {
                let frame = SsmFrame::decode_bytes(bytes)
                    .ok_or_else(|| AppError::Ssm("Invalid SSM handshake frame".into()))?;
                match frame.type_name() {
                    "start_publication" => {
                        // This is a publication control, NOT HandshakeComplete.
                        // Breaking here skipped agent seq=0/1 and corrupted smux.
                        debug!("SSM: publication started; awaiting agent handshake");
                    }
                    "acknowledge" => {
                        let ack = serde_json::from_slice(&frame.payload)
                            .map_err(|error| AppError::Ssm(format!("Invalid SSM ACK: {error}")))?;
                        control(control_tx, WriterControl::Acknowledged(ack)).await?;
                    }
                    "output_stream_data" => {
                        let ack = acknowledgement(&frame)?;
                        if incoming.admit(frame)? {
                            control(control_tx, WriterControl::Send(ack)).await?;
                        }
                        while let Some(frame) = incoming.pop_ready() {
                            match frame.payload_type {
                                PAYLOAD_TYPE_HANDSHAKE_REQ => {
                                    let (response, needs_keepalive) = handshake_response(&frame.payload)?;
                                    keepalive = needs_keepalive;
                                    saw_request = true;
                                    input_tx.send(InputPacket {
                                        payload_type: PAYLOAD_TYPE_HANDSHAKE_RESP,
                                        payload: Bytes::from(response),
                                    }).await.map_err(|_| AppError::Ssm("SSM handshake writer stopped".into()))?;
                                }
                                PAYLOAD_TYPE_HANDSHAKE_COMPLETE if saw_request => {
                                    info!("SSM: agent handshake complete");
                                    return Ok(HandshakeState { incoming, keepalive });
                                }
                                _ => return Err(AppError::Ssm(
                                    "Unexpected SSM payload before port-session handshake completed".into())),
                            }
                        }
                    }
                    "channel_closed" => {
                        return Err(AppError::Ssm("SSM channel closed during handshake".into()))
                    }
                    _ => {}
                }
            }
            Message::Ping(payload) => {
                control(control_tx, WriterControl::Send(Message::Pong(payload))).await?
            }
            Message::Close(_) => {
                return Err(AppError::Ssm(
                    "SSM WebSocket closed during handshake".into(),
                ))
            }
            _ => {}
        }
    }
    Err(AppError::Ssm("SSM WebSocket ended during handshake".into()))
}

fn handshake_response(payload: &[u8]) -> Result<(Vec<u8>, bool)> {
    let request: HandshakeRequestPayload = serde_json::from_slice(payload)
        .map_err(|error| AppError::Ssm(format!("Invalid SSM handshake request: {error}")))?;
    let version: Vec<u32> = request
        .agent_version
        .as_deref()
        .unwrap_or("")
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| AppError::Ssm("Invalid SSM agent version in handshake".into()))?;
    if version.len() != 4 || version.as_slice() <= [3, 0, 196, 0].as_slice() {
        return Err(AppError::Ssm(
            "Native multiplexed forwarding requires SSM agent newer than 3.0.196.0".into(),
        ));
    }
    // AWS disables smux keepalive on newer agents to preserve idle timeout.
    let keepalive = version.as_slice() <= [3, 1, 1511, 0].as_slice();
    let mut actions = Vec::new();
    let mut port_session = false;
    for action in request.requested_client_actions {
        match action.action_type.as_str() {
            "SessionType" => {
                let session_type = action
                    .action_parameters
                    .as_ref()
                    .and_then(|value| value.get("SessionType"))
                    .and_then(|value| value.as_str());
                if session_type != Some("Port") {
                    return Err(AppError::Ssm(
                        "Native tunnel requires SSM SessionType=Port".into(),
                    ));
                }
                port_session = true;
                actions.push(ProcessedClientAction {
                    action_type: action.action_type,
                    action_status: 1,
                    action_result: serde_json::Value::Null,
                    error: String::new(),
                });
            }
            "KMSEncryption" => return Err(AppError::Ssm(
                "SSM session requires KMS encryption, which the native tunnel does not yet support"
                    .into(),
            )),
            other => {
                return Err(AppError::Ssm(format!(
                    "Unsupported SSM handshake action: {other}"
                )))
            }
        }
    }
    if !port_session {
        return Err(AppError::Ssm(
            "SSM handshake did not request a Port session".into(),
        ));
    }
    let response = serde_json::to_vec(&HandshakeResponsePayload {
        client_version: "1.3.0.0",
        processed_client_actions: actions,
        errors: Vec::new(),
    })
    .map_err(|error| {
        AppError::Ssm(format!(
            "SSM handshake response serialization failed: {error}"
        ))
    })?;
    Ok((response, keepalive))
}

#[cfg(test)]
async fn websocket_reader<S>(
    ws_src: S,
    control_tx: mpsc::Sender<WriterControl>,
    incoming_tx: mpsc::Sender<SsmFrame>,
    incoming: IncomingWindow,
) -> Result<()>
where
    S: Stream<Item = std::result::Result<Message, WsError>> + Unpin,
{
    websocket_reader_with_acks(ws_src, control_tx, incoming_tx, incoming, None).await
}

async fn websocket_reader_with_acks<S>(
    mut ws_src: S,
    control_tx: mpsc::Sender<WriterControl>,
    incoming_tx: mpsc::Sender<SsmFrame>,
    mut incoming: IncomingWindow,
    ack_tx: Option<mpsc::Sender<AckPayload>>,
) -> Result<()>
where
    S: Stream<Item = std::result::Result<Message, WsError>> + Unpin,
{
    loop {
        tokio::select! {
            // Deliver ordered frames when there is capacity, but keep reading ACKs
            // and Ping/control traffic while the TCP dispatcher is backpressured.
            permit = incoming_tx.reserve(), if incoming.has_ready() => {
                let permit = permit.map_err(|_| AppError::Ssm("SSM inbound dispatcher stopped".into()))?;
                permit.send(incoming.pop_ready().unwrap());
            }
            message = ws_src.next() => {
                let Some(message) = message else { return Ok(()); };
                match message.map_err(|error| AppError::Ssm(format!("SSM WebSocket receive failed: {error}")))? {
                    Message::Binary(bytes) => {
                        let frame = SsmFrame::decode_bytes(bytes)
                            .ok_or_else(|| AppError::Ssm("Invalid SSM data-channel frame".into()))?;
                        match frame.type_name() {
                            "output_stream_data" => {
                                let ack = AckPayload::for_frame(&frame);
                                if incoming.admit(frame)? {
                                    try_acknowledgement(&control_tx,ack)?;
                                }
                            }
                            "acknowledge" => {
                                let ack = serde_json::from_slice(&frame.payload)
                                    .map_err(|error| AppError::Ssm(format!("Invalid SSM ACK: {error}")))?;
                                if let Some(tx)=&ack_tx {
                                    tx.send(ack).await.map_err(|_|AppError::Ssm("SSM input-ACK inbox stopped".into()))?;
                                } else {
                                    control(&control_tx, WriterControl::Acknowledged(ack)).await?;
                                }
                            }
                            "channel_closed" => return Ok(()),
                            _ => {}
                        }
                    }
                    Message::Ping(payload) => { try_control(&control_tx, WriterControl::Send(Message::Pong(payload)))?; },
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
        }
    }
}

async fn forward_connection(
    mut tcp: TcpStream,
    peer: std::net::SocketAddr,
    mut smux_stream: crate::transport::smux::SmuxStream,
) {
    let _ = tcp.set_nodelay(true);
    let stream_id = smux_stream.id();
    info!("SSM: forwarding {peer} on smux stream {stream_id}");
    // copy_bidirectional returns on I/O errors and preserves TCP half-close;
    // join!(two copies) previously left the other copy alive after an error.
    let result =
        tokio::io::copy_bidirectional_with_sizes(&mut tcp, &mut smux_stream, 32 * 1024, 32 * 1024)
            .await;
    debug!("SSM: {peer} (smux {stream_id}) finished: {result:?}");
}

// ── SigV4 signing helper ──────────────────────────────────────────────────────

/// Signs a tungstenite WebSocket upgrade request with AWS SigV4.
///
/// The SSM `stream_url` is a plain (non-presigned) WSS URL. The real
/// `session-manager-plugin` signs the HTTP GET upgrade request with
/// SigV4 (service = `ssmmessages`) before opening the WebSocket. Without
/// these auth headers the service accepts the TCP/WS handshake but immediately
/// closes the data channel with `code=1000 reason=Bye`.
fn sign_request_sigv4(
    request: &mut tokio_tungstenite::tungstenite::handshake::client::Request,
    creds: &SessionCredentials,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::time::SystemTime;

    let uri = request.uri().to_string();
    let headers: Vec<_> = request
        .headers()
        .iter()
        .map(|(name, value)| value.to_str().map(|value| (name.as_str(), value)))
        .collect::<std::result::Result<_, _>>()?;
    let identity = creds.credentials.clone().into();
    let settings = SigningSettings::default();
    let params = SigningParams::builder()
        .identity(&identity)
        .region(&creds.region)
        .name("ssmmessages")
        .time(SystemTime::now())
        .settings(settings)
        .build()?;

    // Sign — the signable body for a GET is empty
    let signable =
        SignableRequest::new("GET", &uri, headers.into_iter(), SignableBody::Bytes(&[]))?;

    let (signing_instructions, _sig) = sign(signable, &params.into())?.into_parts();

    // Apply the signed headers back to the tungstenite request
    for (name, value) in signing_instructions.headers() {
        let hv = HeaderValue::from_str(value)?;
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::header::HeaderName::from_bytes(name.as_bytes())?,
            hv,
        );
    }

    info!(
        "SSM: WebSocket request signed with SigV4 (ssmmessages/{})",
        creds.region
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_msg_type_padding_and_name() {
        let buf = msg_type("input_stream_data");
        assert_eq!(&buf[..17], b"input_stream_data");
        assert_eq!(&buf[17..], &[b' '; 15]);

        let frame = SsmFrame::new(buf, 42, FLAG_NONE, PAYLOAD_TYPE_DATA, vec![1, 2, 3]);
        assert_eq!(frame.type_name(), "input_stream_data");
    }

    #[test]
    fn test_ssm_frame_encode_decode_roundtrip() {
        let payload = b"Hello SSM Session Manager via Native Rust!".to_vec();
        let frame = SsmFrame::new(
            msg_type("output_stream_data"),
            105,
            FLAG_NONE,
            PAYLOAD_TYPE_DATA,
            payload.clone(),
        );

        let encoded = frame.encode();
        assert_eq!(encoded.len(), HEADER_SIZE + payload.len());

        let decoded = SsmFrame::decode(&encoded).expect("frame should decode");
        assert_eq!(decoded.type_name(), "output_stream_data");
        assert_eq!(decoded.sequence_number, 105);
        assert_eq!(decoded.flags, FLAG_NONE);
        assert_eq!(decoded.payload_type, PAYLOAD_TYPE_DATA);
        assert_eq!(decoded.payload, payload);
        assert_eq!(decoded.message_id_str(), frame.message_id_str());
    }

    #[test]
    fn test_ssm_frame_decode_truncated() {
        // Less than header size
        let short = vec![0u8; 119];
        assert!(SsmFrame::decode(&short).is_none());

        // Header claims payload length, but payload is truncated
        let payload = vec![0xAA; 50];
        let frame = SsmFrame::new(
            msg_type("acknowledge"),
            1,
            FLAG_NONE,
            PAYLOAD_TYPE_NONE,
            payload,
        );
        let mut encoded = frame.encode();
        encoded.truncate(encoded.len() - 10);
        assert!(SsmFrame::decode(&encoded).is_none());
    }

    #[test]
    fn test_token_frame_serialization() {
        let token = TokenFrame {
            message_schema_version: "1.0",
            request_id: "12345678-1234-1234-1234-123456789abc".to_string(),
            token_value: "test-auth-token-xyz".to_string(),
            client_id: "87654321-4321-4321-4321-cba987654321".to_string(),
            client_version: "1.3.0.0",
        };
        let json = serde_json::to_string(&token).unwrap();
        assert!(json.contains("\"MessageSchemaVersion\":\"1.0\""));
        assert!(json.contains("\"RequestId\":\"12345678-1234-1234-1234-123456789abc\""));
        assert!(json.contains("\"TokenValue\":\"test-auth-token-xyz\""));
        assert!(json.contains("\"ClientId\":\"87654321-4321-4321-4321-cba987654321\""));
        assert!(json.contains("\"ClientVersion\":\"1.3.0.0\""));
    }

    #[test]
    fn test_ack_payload_serialization() {
        let ack = AckPayload {
            acknowledged_message_type: AckMessageType::OutputStreamData,
            acknowledged_message_id: Uuid::from_u128(123),
            acknowledged_message_sequence_number: 99,
            is_sequential_message: true,
        };
        let json = serde_json::to_string(&ack).unwrap();
        assert!(json.contains("\"AcknowledgedMessageType\":\"output_stream_data\""));
        assert!(json.contains("\"AcknowledgedMessageId\":\"00000000-0000-0000-0000-00000000007b\""));
        assert!(json.contains("\"AcknowledgedMessageSequenceNumber\":99"));
        assert!(json.contains("\"IsSequentialMessage\":true"));
    }
}

#[cfg(test)]
#[path = "tests/tunnel_tests.rs"]
mod transport_tests;

#[cfg(test)]
#[path = "tests/writer_bench.rs"]
mod writer_benchmark;
