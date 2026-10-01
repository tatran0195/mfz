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
//! 4. Wait for `start_publication` from the server
//! 5. Bind a TCP listener on `local_port`; relay bidirectional data between
//!    the accepted TCP socket and the WebSocket using SSM binary frames
//!
//! # SSM binary frame layout (all fields big-endian)
//!
//! ```text
//! Offset  Size  Field
//! ──────  ────  ──────────────────────────────────
//!      0     4  HeaderLength  (always 120)
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

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use aws_sigv4::{
    http_request::{sign, SignableBody, SignableRequest, SigningSettings},
    sign::v4::SigningParams,
};
use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{oneshot, Mutex},
};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::error::{AppError, Result};

// ── Wire constants ─────────────────────────────────────────────────────────────

const HEADER_SIZE: usize = 120;
const SCHEMA_VERSION: u32 = 1;
const FLAG_NONE: u64 = 0;
const FLAG_FIN: u64 = 2;
const FLAG_ACK: u64 = 3; // SYN | FIN used by session-manager-plugin for AcknowledgeMessage
const PAYLOAD_TYPE_DATA: u32 = 1;
const PAYLOAD_TYPE_NONE: u32 = 0;
const PAYLOAD_TYPE_HANDSHAKE_REQ: u32 = 5;
const PAYLOAD_TYPE_HANDSHAKE_RESP: u32 = 6;
const PAYLOAD_TYPE_HANDSHAKE_COMPLETE: u32 = 7;

// ── Message type helper ────────────────────────────────────────────────────────

type MsgTypeBuf = [u8; 32];

fn msg_type(s: &str) -> MsgTypeBuf {
    let mut buf = [b' '; 32];
    let bytes = s.as_bytes();
    buf[..bytes.len().min(32)].copy_from_slice(&bytes[..bytes.len().min(32)]);
    buf
}

// ── SSM binary frame ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct SsmFrame {
    msg_type: MsgTypeBuf,
    sequence_number: i64,
    flags: u64,
    message_id: [u8; 16],
    payload_type: u32,
    payload: Vec<u8>,
}

impl SsmFrame {
    fn new(
        msg_type: MsgTypeBuf,
        seq: i64,
        flags: u64,
        payload_type: u32,
        payload: Vec<u8>,
    ) -> Self {
        let id = Uuid::new_v4();
        let bytes = id.as_bytes();
        let mut message_id = [0u8; 16];
        // AWS session-manager wire format swaps the two 64-bit halves of the UUID:
        // offset 64..72 = UUID[8..16], offset 72..80 = UUID[0..8]
        message_id[0..8].copy_from_slice(&bytes[8..16]);
        message_id[8..16].copy_from_slice(&bytes[0..8]);
        Self {
            msg_type,
            sequence_number: seq,
            flags,
            message_id,
            payload_type,
            payload,
        }
    }

    fn encode(&self) -> Vec<u8> {
        let created_date = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let digest: [u8; 32] = Sha256::digest(&self.payload).into();
        let mut buf = Vec::with_capacity(HEADER_SIZE + self.payload.len());

        buf.extend_from_slice(&116u32.to_be_bytes()); // HeaderLength (116 = offset of PayloadLength)
        buf.extend_from_slice(&self.msg_type); // MessageType
        buf.extend_from_slice(&SCHEMA_VERSION.to_be_bytes()); // SchemaVersion
        buf.extend_from_slice(&created_date.to_be_bytes()); // CreatedDate
        buf.extend_from_slice(&self.sequence_number.to_be_bytes()); // SequenceNumber
        buf.extend_from_slice(&self.flags.to_be_bytes()); // Flags
        buf.extend_from_slice(&self.message_id); // MessageId
        buf.extend_from_slice(&digest); // PayloadDigest
        buf.extend_from_slice(&self.payload_type.to_be_bytes()); // PayloadType
        buf.extend_from_slice(&(self.payload.len() as u32).to_be_bytes()); // PayloadLength
        buf.extend_from_slice(&self.payload); // Payload
        buf
    }

    fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < HEADER_SIZE {
            return None;
        }

        let header_len = u32::from_be_bytes(data[0..4].try_into().ok()?) as usize;

        let mut msg_type = [0u8; 32];
        msg_type.copy_from_slice(&data[4..36]);

        let sequence_number = i64::from_be_bytes(data[48..56].try_into().ok()?);
        let flags = u64::from_be_bytes(data[56..64].try_into().ok()?);

        let mut message_id = [0u8; 16];
        message_id.copy_from_slice(&data[64..80]);

        let payload_type = u32::from_be_bytes(data[112..116].try_into().ok()?);
        let payload_len = u32::from_be_bytes(data[116..120].try_into().ok()?) as usize;

        let payload_offset = if header_len == 116 { 120 } else { header_len };
        if data.len() < payload_offset + payload_len {
            return None;
        }
        let payload = data[payload_offset..payload_offset + payload_len].to_vec();

        Some(Self {
            msg_type,
            sequence_number,
            flags,
            message_id,
            payload_type,
            payload,
        })
    }

    fn type_name(&self) -> &str {
        std::str::from_utf8(&self.msg_type).unwrap_or("").trim_end()
    }

    fn message_id_str(&self) -> String {
        let mut uuid_bytes = [0u8; 16];
        // AWS session-manager wire format swaps the two 64-bit halves of the UUID:
        // wire[0..8] = UUID[8..16], wire[8..16] = UUID[0..8]
        uuid_bytes[0..8].copy_from_slice(&self.message_id[8..16]);
        uuid_bytes[8..16].copy_from_slice(&self.message_id[0..8]);
        Uuid::from_bytes(uuid_bytes).to_string()
    }
}

// ── JSON payloads ──────────────────────────────────────────────────────────────

/// Initial auth token sent as the first WebSocket text frame.
/// Field names and types match OpenDataChannelInput in session-manager-plugin:
/// - MessageSchemaVersion: "1.0"
/// - RequestId: UUID string
/// - TokenValue: SSM session token
/// - ClientId: UUID string
/// - ClientVersion: plugin version (e.g. "1.3.0.0")
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct TokenFrame {
    message_schema_version: &'static str,
    request_id: String,
    token_value: String,
    client_id: String,
    client_version: &'static str,
}

/// Payload inside every `acknowledge` SSM frame.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct AckPayload {
    acknowledged_message_type: String,
    acknowledged_message_id: String,
    acknowledged_message_sequence_number: i64,
    is_sequential_message: bool,
}

/// Handshake request payload sent by the SSM agent (payload_type = 5).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HandshakeRequestPayload {
    #[allow(dead_code)]
    agent_version: Option<String>,
    requested_client_actions: Vec<RequestedClientAction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RequestedClientAction {
    action_type: String,
    #[allow(dead_code)]
    action_parameters: Option<serde_json::Value>,
}

/// Handshake response payload sent back to SSM agent (payload_type = 6).
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct HandshakeResponsePayload {
    client_version: &'static str,
    processed_client_actions: Vec<ProcessedClientAction>,
    errors: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ProcessedClientAction {
    action_type: String,
    action_status: u32, // 1 = Success, 2 = Failed, 3 = Unsupported
    action_result: serde_json::Value,
    error: String,
}

// ── Public tunnel handle ───────────────────────────────────────────────────────

/// Returned by [`start_port_forward_tunnel`]; drop or call [`stop`] to clean up.
pub struct SsmTunnelHandle {
    pub(crate) shutdown_tx: Option<oneshot::Sender<()>>,
    pub(crate) bound_rx: Option<oneshot::Receiver<()>>,
    pub(crate) task: tokio::task::JoinHandle<()>,
}

impl SsmTunnelHandle {
    pub fn is_running(&self) -> bool {
        !self.task.is_finished()
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.task.abort();
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
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
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
/// Returns as soon as the WebSocket is connected and the TCP listener is bound;
/// actual data forwarding happens in a background Tokio task.
pub async fn start_port_forward_tunnel(
    stream_url: String,
    token_value: String,
    local_port: u16,
    _remote_port: u16, // conveyed to SSM via StartSession parameters
    creds: SessionCredentials,
) -> Result<SsmTunnelHandle> {
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (bound_tx, bound_rx) = oneshot::channel::<()>();

    let task = tokio::spawn(async move {
        if let Err(e) = run_tunnel(
            stream_url,
            token_value,
            local_port,
            shutdown_rx,
            bound_tx,
            creds,
        )
        .await
        {
            error!("SSM native tunnel exited with error: {e}");
        }
    });

    Ok(SsmTunnelHandle {
        shutdown_tx: Some(shutdown_tx),
        bound_rx: Some(bound_rx),
        task,
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
) -> Result<()> {
    // ── 1. Open WSS connection with SigV4-signed upgrade headers ─────────────
    // The stream_url returned by StartSession is NOT presigned. The SSM service
    // requires the WebSocket HTTP upgrade to carry a standard SigV4 Authorization
    // header (service=ssmmessages). Without it the server closes immediately
    // with WebSocket code=1000 reason="Bye".
    info!(
        "SSM: connecting WebSocket → {}",
        &stream_url[..stream_url.find('?').unwrap_or(stream_url.len())]
    );

    let mut request = stream_url
        .as_str()
        .into_client_request()
        .map_err(|e| AppError::Ssm(format!("Invalid stream URL: {e}")))?;

    // Sign the HTTP GET upgrade request with SigV4
    sign_request_sigv4(&mut request, &creds)
        .map_err(|e| AppError::Ssm(format!("SigV4 signing failed: {e}")))?;

    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| AppError::Ssm(format!("WebSocket connect failed: {e}")))?;

    let (ws_sink, mut ws_src) = ws.split();
    // Wrap the sink in Arc<Mutex<>> so the WebSocket receiver task and the
    // TCP→WebSocket path can both write without moving ownership.
    let ws_sink = Arc::new(Mutex::new(ws_sink));

    // ── 2. Auth token ────────────────────────────────────────────────────────
    let token_json = serde_json::to_string(&TokenFrame {
        message_schema_version: "1.0",
        request_id: Uuid::new_v4().to_string(),
        token_value: token_value.clone(),
        client_id: Uuid::new_v4().to_string(),
        client_version: "1.3.0.0",
    })
    .map_err(|e| AppError::Ssm(format!("Token serialise error: {e}")))?;

    info!("SSM: sending token frame (len={})", token_json.len());
    debug!("SSM: token frame body: {token_json}");

    ws_sink
        .lock()
        .await
        .send(Message::Text(token_json))
        .await
        .map_err(|e| AppError::Ssm(format!("Token send error: {e}")))?;

    info!("SSM: auth token sent, awaiting handshake and server ready…");

    // Sequence counter shared across handshake, TCP→WebSocket write, and WebSocket→TCP ack paths.
    let seq = Arc::new(AtomicI64::new(0));
    let mut next_expected_seq = 2i64;

    // ── 3. Handshake and await server ready ──────────────────────────────────
    loop {
        match ws_src.next().await {
            Some(Ok(Message::Binary(data))) => {
                if let Some(f) = SsmFrame::decode(&data) {
                    let t = f.type_name();
                    info!(
                        "SSM ← {t} (seq={}, payload_type={}, len={})",
                        f.sequence_number,
                        f.payload_type,
                        f.payload.len()
                    );

                    if t == "start_publication" {
                        info!("SSM: server ready (start_publication)");
                        break;
                    }

                    if t == "output_stream_data" {
                        // Acknowledge the message immediately (seq=0, flags=FLAG_ACK matching session-manager-plugin)
                        let ack_payload = serde_json::to_vec(&AckPayload {
                            acknowledged_message_type: t.to_string(),
                            acknowledged_message_id: f.message_id_str(),
                            acknowledged_message_sequence_number: f.sequence_number,
                            is_sequential_message: true,
                        })
                        .unwrap_or_default();

                        let ack = SsmFrame::new(
                            msg_type("acknowledge"),
                            0, // SequenceNumber is always 0 for acknowledge messages
                            FLAG_ACK,
                            PAYLOAD_TYPE_NONE,
                            ack_payload,
                        );
                        let _ = ws_sink
                            .lock()
                            .await
                            .send(Message::Binary(ack.encode()))
                            .await;
                        info!(
                            "SSM: acknowledged {} seq={} id={}",
                            t,
                            f.sequence_number,
                            f.message_id_str()
                        );

                        if f.payload_type == PAYLOAD_TYPE_HANDSHAKE_REQ {
                            info!(
                                "SSM: received HandshakeRequest: {}",
                                String::from_utf8_lossy(&f.payload)
                            );
                            let mut processed_actions = Vec::new();
                            let errors = Vec::new();

                            if let Ok(req) =
                                serde_json::from_slice::<HandshakeRequestPayload>(&f.payload)
                            {
                                for action in req.requested_client_actions {
                                    if action.action_type == "SessionType" {
                                        processed_actions.push(ProcessedClientAction {
                                            action_type: action.action_type,
                                            action_status: 1, // Success
                                            action_result: serde_json::Value::Null,
                                            error: String::new(),
                                        });
                                    } else if action.action_type == "KMSEncryption" {
                                        return Err(AppError::Ssm(
                                            "SSM session requires KMS encryption. \
                                             Native tunnel does not support encrypted sessions yet. \
                                             Either remove KmsKeyId from the SSM Session document, \
                                             or install session-manager-plugin as a fallback."
                                                .into(),
                                        ));
                                    } else {
                                        processed_actions.push(ProcessedClientAction {
                                            action_type: action.action_type,
                                            action_status: 3, // Unsupported
                                            action_result: serde_json::Value::Null,
                                            error: "Unsupported action".to_string(),
                                        });
                                    }
                                }
                            }

                            let resp_payload = serde_json::to_vec(&HandshakeResponsePayload {
                                client_version: "1.3.0.0",
                                processed_client_actions: processed_actions,
                                errors,
                            })
                            .map_err(|e| {
                                AppError::Ssm(format!("HandshakeResponse serialize error: {e}"))
                            })?;

                            let resp_seq = seq.fetch_add(1, Ordering::SeqCst);
                            let resp_frame = SsmFrame::new(
                                msg_type("input_stream_data"),
                                resp_seq,
                                FLAG_NONE,
                                PAYLOAD_TYPE_HANDSHAKE_RESP,
                                resp_payload.clone(),
                            );
                            ws_sink
                                .lock()
                                .await
                                .send(Message::Binary(resp_frame.encode()))
                                .await
                                .map_err(|e| {
                                    AppError::Ssm(format!("HandshakeResponse send error: {e}"))
                                })?;
                            info!(
                                "SSM: sent HandshakeResponse (seq={resp_seq}): {}",
                                String::from_utf8_lossy(&resp_payload)
                            );
                        } else if f.payload_type == PAYLOAD_TYPE_HANDSHAKE_COMPLETE {
                            info!(
                                "SSM: HandshakeComplete received and acknowledged: {}",
                                String::from_utf8_lossy(&f.payload)
                            );
                            info!("SSM: server handshake complete, proceeding to bind local port");
                            next_expected_seq = f.sequence_number + 1;
                            break;
                        }
                    } else if t == "channel_closed" {
                        info!(
                            "SSM ← channel_closed: {}",
                            String::from_utf8_lossy(&f.payload)
                        );
                    }
                }
            }
            Some(Ok(Message::Text(txt))) => {
                info!("SSM ← text frame: {txt}");
            }
            Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                // Ignore keepalive frames silently.
            }
            Some(Ok(Message::Close(reason))) => {
                let reason_str = reason
                    .map(|r| format!(" code={} reason={}", r.code, r.reason))
                    .unwrap_or_default();
                return Err(AppError::Ssm(format!(
                    "WebSocket closed before start_publication{reason_str}"
                )));
            }
            None => {
                return Err(AppError::Ssm(
                    "WebSocket closed before start_publication".into(),
                ));
            }
            Some(Err(e)) => return Err(AppError::Ssm(format!("WebSocket error: {e}"))),
            Some(Ok(_)) => {} // future frame types
        }
    }

    // ── 4. Bind local TCP listener ───────────────────────────────────────────
    let listener = TcpListener::bind(format!("127.0.0.1:{local_port}"))
        .await
        .map_err(|e| AppError::Ssm(format!("TCP bind :{local_port} failed: {e}")))?;

    info!("SSM: port forwarder listening on 127.0.0.1:{local_port}");
    let _ = bound_tx.send(());

    // ── 5. smux Client relay ──────────────────────────────────────────────────
    //
    // AWS SSM port-forwarding uses smux v1 (xtaci/smux) inside the
    // output_stream_data / input_stream_data SSM frame payloads.
    //
    // As in AWS Session Manager Plugin (MuxPortForwarding), we are the smux CLIENT:
    // When a local TCP client (mstsc) connects to 127.0.0.1:local_port, we call
    // `smux_session.open_stream()`, which assigns an odd stream ID (1, 3, 5...) and
    // sends a CMD_SYN frame to the remote SSM agent.
    // Bidirectional data is then pumped between the TCP socket and the smux stream.

    let (smux_session, mut outbound_frame_rx) = crate::smux::SmuxSession::new();

    // Channel: inner shutdown (WebSocket closed) → outer select
    let (ws_done_tx, mut ws_done_rx) = oneshot::channel::<()>();

    // ── smux→WebSocket outbound forward task ──────────────────────────────────
    let ws_sink_smux_fwd = Arc::clone(&ws_sink);
    let seq_fwd = Arc::clone(&seq);
    let smux_session_fwd = smux_session.clone();
    tokio::spawn(async move {
        // AWS Systems Manager imposes a strict 1024-byte payload limit
        // (config.StreamDataPayloadSize = 1024) on input_stream_data messages.
        // Frames larger than 1024 bytes (e.g. TLS certificates, NLA credentials)
        // must be sliced into sequential 1024-byte chunks or SSM Agent drops them.
        const MAX_STREAM_DATA_PAYLOAD: usize = 1024;

        while let Some(frame_bytes) = outbound_frame_rx.recv().await {
            let mut send_err = false;
            for chunk in frame_bytes.chunks(MAX_STREAM_DATA_PAYLOAD) {
                let s = seq_fwd.fetch_add(1, Ordering::SeqCst);
                let frame = SsmFrame::new(
                    msg_type("input_stream_data"),
                    s,
                    FLAG_NONE,
                    PAYLOAD_TYPE_DATA,
                    chunk.to_vec(),
                );
                let mut sink = ws_sink_smux_fwd.lock().await;
                if let Err(e) = sink.send(Message::Binary(frame.encode())).await {
                    error!("SSM: WebSocket send error (smux outbound): {e}");
                    send_err = true;
                    break;
                }
            }
            if send_err {
                break;
            }
        }
        smux_session_fwd.close();
        debug!("SSM: smux outbound forward task finished");
    });

    // ── WebSocket receive task ────────────────────────────────────────────────
    let ws_sink_rx = Arc::clone(&ws_sink);
    let smux_session_rx = smux_session.clone();

    tokio::spawn(async move {
        let mut smux_in_buf = BytesMut::new();
        let mut expected_seq = next_expected_seq;
        let mut incoming_buffer: BTreeMap<i64, SsmFrame> = BTreeMap::new();

        while let Some(msg) = ws_src.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    let Some(frame) = SsmFrame::decode(&data) else {
                        continue;
                    };
                    let t = frame.type_name();
                    debug!(
                        "SSM ← {t}  seq={} len={}",
                        frame.sequence_number,
                        frame.payload.len()
                    );

                    match t {
                        "output_stream_data" => {
                            // Acknowledge every output_stream_data frame (matching session-manager-plugin)
                            let ack_payload = serde_json::to_vec(&AckPayload {
                                acknowledged_message_type: t.to_string(),
                                acknowledged_message_id: frame.message_id_str(),
                                acknowledged_message_sequence_number: frame.sequence_number,
                                is_sequential_message: true,
                            })
                            .unwrap_or_default();

                            let ack = SsmFrame::new(
                                msg_type("acknowledge"),
                                0,
                                FLAG_ACK,
                                PAYLOAD_TYPE_NONE,
                                ack_payload,
                            );
                            let _ = ws_sink_rx
                                .lock()
                                .await
                                .send(Message::Binary(ack.encode()))
                                .await;
                            debug!(
                                "SSM: acknowledged {} seq={} id={}",
                                t,
                                frame.sequence_number,
                                frame.message_id_str()
                            );

                            // Discard duplicates from remote retransmission (RTO)
                            if frame.sequence_number < expected_seq {
                                debug!(
                                    "SSM ← duplicate output_stream_data (seq={}, expected={}); discarded",
                                    frame.sequence_number, expected_seq
                                );
                                continue;
                            }

                            // Buffer out-of-order future messages
                            if frame.sequence_number > expected_seq {
                                debug!(
                                    "SSM ← out-of-order output_stream_data (seq={}, expected={}); buffered",
                                    frame.sequence_number, expected_seq
                                );
                                incoming_buffer.insert(frame.sequence_number, frame);
                                continue;
                            }

                            // frame.sequence_number == expected_seq:
                            // Process this frame and any contiguous buffered frames
                            let mut to_process = vec![frame];
                            expected_seq += 1;
                            while let Some(next_frame) = incoming_buffer.remove(&expected_seq) {
                                to_process.push(next_frame);
                                expected_seq += 1;
                            }

                            let mut exit_rx = false;
                            for f in to_process {
                                if f.payload_type == PAYLOAD_TYPE_DATA && !f.payload.is_empty() {
                                    trace!(
                                        "SSM ← output_stream_data (seq={}): len={}",
                                        f.sequence_number,
                                        f.payload.len()
                                    );
                                    smux_in_buf.extend_from_slice(&f.payload);
                                    if let Err(problem) = smux_session_rx.dispatch(&mut smux_in_buf)
                                    {
                                        warn!("SSM: smux decode error: {problem}");
                                        exit_rx = true;
                                        break;
                                    }
                                } else if f.payload_type == 10 {
                                    // Control flag payload (1=DisconnectToPort, 2=TerminateSession, 3=ConnectToPortError)
                                    let flag = if f.payload.len() >= 4 {
                                        u32::from_be_bytes(
                                            f.payload[0..4].try_into().unwrap_or_default(),
                                        )
                                    } else {
                                        0
                                    };
                                    warn!("SSM ← flag from remote: {flag} (1=DisconnectToPort, 2=TerminateSession, 3=ConnectToPortError)");
                                    if flag == 1 || flag == 2 || flag == 3 {
                                        exit_rx = true;
                                        break;
                                    }
                                }

                                if f.flags & FLAG_FIN != 0 {
                                    info!("SSM: remote sent FIN on seq={}", f.sequence_number);
                                    exit_rx = true;
                                    break;
                                }
                            }
                            if exit_rx {
                                break;
                            }
                        }
                        "channel_closed" => {
                            info!("SSM: channel closed by remote");
                            break;
                        }
                        other => debug!("SSM: ignoring message type '{other}'"),
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("SSM: WebSocket close frame");
                    break;
                }
                Err(e) => {
                    error!("SSM WebSocket recv error: {e}");
                    break;
                }
                _ => {}
            }
        }
        smux_session_rx.close();
        let _ = ws_done_tx.send(());
        info!("SSM tunnel: WebSocket receive task ended");
    });

    // ── TCP accept loop ───────────────────────────────────────────────────────
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                info!("SSM tunnel: shutdown requested");
                break;
            }
            _ = &mut ws_done_rx => {
                info!("SSM tunnel: WebSocket closed, exiting");
                break;
            }
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((tcp, peer)) => {
                        info!("SSM tunnel: TCP client connected from {peer}");
                        match smux_session.open_stream() {
                            Ok(smux_stream) => {
                                tokio::spawn(forward_connection(tcp, peer, smux_stream));
                            }
                            Err(e) => {
                                error!("SSM tunnel: failed to open smux stream for {peer}: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        error!("SSM tunnel: TCP accept error: {e}");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Pump data between an accepted TCP connection (mstsc) and its logical smux stream.
async fn forward_connection(
    tcp: TcpStream,
    peer: std::net::SocketAddr,
    smux_stream: crate::smux::SmuxStream,
) {
    if let Err(e) = tcp.set_nodelay(true) {
        debug!("SSM tunnel: failed to set TCP_NODELAY for {peer}: {e}");
    }
    let stream_id = smux_stream.id();
    info!("SSM tunnel: forwarding connection from {peer} on smux stream {stream_id}");

    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let (mut smux_read, mut smux_write) = tokio::io::split(smux_stream);

    let outbound = async {
        let res = tokio::io::copy(&mut tcp_read, &mut smux_write).await;
        let _ = smux_write.shutdown().await;
        res
    };

    let inbound = async {
        let res = tokio::io::copy(&mut smux_read, &mut tcp_write).await;
        let _ = tcp_write.shutdown().await;
        res
    };

    let (sent, received) = tokio::join!(outbound, inbound);
    debug!(
        "SSM tunnel: connection {peer} (smux {stream_id}) finished: sent={sent:?}, received={received:?}"
    );
    info!("SSM tunnel: TCP client {peer} disconnected");
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

    // Build a standard http::Request from the tungstenite request so the
    // aws-sigv4 crate can inspect URI, method, and headers.
    let uri = request.uri().to_string();
    let mut http_req = http::Request::builder().method("GET").uri(&uri).body(())?;

    // Copy existing headers (e.g. Host set by tungstenite)
    for (name, value) in request.headers() {
        http_req.headers_mut().insert(name.clone(), value.clone());
    }

    // Build signing parameters
    let credentials = aws_credential_types::Credentials::new(
        creds.access_key_id.clone(),
        creds.secret_access_key.clone(),
        Some(creds.session_token.clone()),
        None,
        "ssm_tunnel",
    );
    let identity = credentials.into();
    let settings = SigningSettings::default();
    let params = SigningParams::builder()
        .identity(&identity)
        .region(&creds.region)
        .name("ssmmessages")
        .time(SystemTime::now())
        .settings(settings)
        .build()?;

    // Sign — the signable body for a GET is empty
    let signable = SignableRequest::new(
        "GET",
        &uri,
        http_req
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str(), v.to_str().unwrap_or(""))),
        SignableBody::Bytes(&[]),
    )?;

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
            acknowledged_message_type: "output_stream_data".to_string(),
            acknowledged_message_id: "test-id-123".to_string(),
            acknowledged_message_sequence_number: 99,
            is_sequential_message: true,
        };
        let json = serde_json::to_string(&ack).unwrap();
        assert!(json.contains("\"AcknowledgedMessageType\":\"output_stream_data\""));
        assert!(json.contains("\"AcknowledgedMessageId\":\"test-id-123\""));
        assert!(json.contains("\"AcknowledgedMessageSequenceNumber\":99"));
        assert!(json.contains("\"IsSequentialMessage\":true"));
    }
}
