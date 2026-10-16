//! AWS SSM streaming wire codec. Immutable Bytes keep retransmission and
//! receive slicing cheap without changing the one-message-per-WebSocket contract.
use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

// ── Wire constants ─────────────────────────────────────────────────────────────

pub(crate) const HEADER_SIZE: usize = 120;
pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const FLAG_NONE: u64 = 0;
pub(crate) const FLAG_FIN: u64 = 2;
pub(crate) const FLAG_ACK: u64 = 3; // SYN | FIN used by session-manager-plugin for AcknowledgeMessage
pub(crate) const PAYLOAD_TYPE_DATA: u32 = 1;
pub(crate) const PAYLOAD_TYPE_NONE: u32 = 0;
pub(crate) const PAYLOAD_TYPE_HANDSHAKE_REQ: u32 = 5;
pub(crate) const PAYLOAD_TYPE_HANDSHAKE_RESP: u32 = 6;
pub(crate) const PAYLOAD_TYPE_HANDSHAKE_COMPLETE: u32 = 7;

// ── Message type helper ────────────────────────────────────────────────────────

pub(crate) type MsgTypeBuf = [u8; 32];

pub(crate) fn msg_type(s: &str) -> MsgTypeBuf {
    let mut buf = [b' '; 32];
    let bytes = s.as_bytes();
    buf[..bytes.len().min(32)].copy_from_slice(&bytes[..bytes.len().min(32)]);
    buf
}

// ── SSM binary frame ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct SsmFrame {
    pub(crate) msg_type: MsgTypeBuf,
    pub(crate) sequence_number: i64,
    pub(crate) flags: u64,
    pub(crate) message_id: [u8; 16],
    pub(crate) payload_type: u32,
    pub(crate) payload: Bytes,
}

impl SsmFrame {
    pub(crate) fn new(
        msg_type: MsgTypeBuf,
        seq: i64,
        flags: u64,
        payload_type: u32,
        payload: impl Into<Bytes>,
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
            payload: payload.into(),
        }
    }

    pub(crate) fn encode(&self) -> Bytes {
        let created_date = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let digest: [u8; 32] = Sha256::digest(&self.payload).into();
        let mut buf = BytesMut::with_capacity(HEADER_SIZE + self.payload.len());

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
        buf.freeze()
    }

    #[cfg(test)]
    pub(crate) fn decode(data: &[u8]) -> Option<Self> {
        Self::decode_bytes(Bytes::copy_from_slice(data))
    }

    pub(crate) fn decode_bytes(data: Bytes) -> Option<Self> {
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
        let type_name = std::str::from_utf8(&msg_type)
            .ok()?
            .trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
        // MGS publication controls may have zero/unused metadata. The AWS
        // plugin explicitly exempts these controls from message validation.
        if matches!(type_name, "start_publication" | "pause_publication") {
            return Some(Self {
                msg_type,
                sequence_number,
                flags,
                message_id,
                payload_type,
                payload: Bytes::new(),
            });
        }

        let payload_offset: usize = match header_len {
            116 | 120 => HEADER_SIZE,
            _ => return None,
        };
        let payload_end = payload_offset.checked_add(payload_len)?;
        if data.len() != payload_end
            || u32::from_be_bytes(data[36..40].try_into().ok()?) != SCHEMA_VERSION
        {
            return None;
        }
        let payload = data.slice(payload_offset..payload_end);
        if !payload.is_empty() && Sha256::digest(&payload)[..] != data[80..112] {
            return None;
        }

        Some(Self {
            msg_type,
            sequence_number,
            flags,
            message_id,
            payload_type,
            payload,
        })
    }

    pub(crate) fn type_name(&self) -> &str {
        std::str::from_utf8(&self.msg_type)
            .unwrap_or("")
            .trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '\0')
    }

    pub(crate) fn message_uuid(&self) -> Uuid {
        let mut uuid_bytes = [0u8; 16];
        // AWS session-manager wire format swaps the two 64-bit halves of the UUID:
        // wire[0..8] = UUID[8..16], wire[8..16] = UUID[0..8]
        uuid_bytes[0..8].copy_from_slice(&self.message_id[8..16]);
        uuid_bytes[8..16].copy_from_slice(&self.message_id[0..8]);
        Uuid::from_bytes(uuid_bytes)
    }

    #[cfg(test)]
    pub(crate) fn message_id_str(&self) -> String {
        self.message_uuid().to_string()
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
pub(crate) struct TokenFrame {
    pub(crate) message_schema_version: &'static str,
    pub(crate) request_id: String,
    pub(crate) token_value: String,
    pub(crate) client_id: String,
    pub(crate) client_version: &'static str,
}

/// Input/output ACK types are fixed protocol strings, not heap strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AckMessageType {
    InputStreamData,
    OutputStreamData,
    #[serde(other)]
    Other,
}

impl From<&str> for AckMessageType {
    fn from(value: &str) -> Self {
        match value {
            "input_stream_data" => Self::InputStreamData,
            "output_stream_data" => Self::OutputStreamData,
            _ => Self::Other,
        }
    }
}

pub(crate) const MAX_STREAM_DATA_PAYLOAD: usize = 1024;
pub(crate) const MAX_INCOMING_PAYLOAD: usize = 64 * 1024;

/// Payload inside every `acknowledge` SSM frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct AckPayload {
    pub(crate) acknowledged_message_type: AckMessageType,
    pub(crate) acknowledged_message_id: Uuid,
    pub(crate) acknowledged_message_sequence_number: i64,
    pub(crate) is_sequential_message: bool,
}

impl AckPayload {
    pub(crate) fn for_frame(frame: &SsmFrame) -> Self {
        Self {
            acknowledged_message_type: frame.type_name().into(),
            acknowledged_message_id: frame.message_uuid(),
            acknowledged_message_sequence_number: frame.sequence_number,
            is_sequential_message: true,
        }
    }
}

/// Handshake request payload sent by the SSM agent (payload_type = 5).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct HandshakeRequestPayload {
    pub(crate) agent_version: Option<String>,
    pub(crate) requested_client_actions: Vec<RequestedClientAction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct RequestedClientAction {
    pub(crate) action_type: String,
    pub(crate) action_parameters: Option<serde_json::Value>,
}

/// Handshake response payload sent back to SSM agent (payload_type = 6).
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct HandshakeResponsePayload {
    pub(crate) client_version: &'static str,
    pub(crate) processed_client_actions: Vec<ProcessedClientAction>,
    pub(crate) errors: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ProcessedClientAction {
    pub(crate) action_type: String,
    pub(crate) action_status: u32, // 1 = Success, 2 = Failed, 3 = Unsupported
    pub(crate) action_result: serde_json::Value,
    pub(crate) error: String,
}
