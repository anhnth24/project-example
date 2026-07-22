use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Cursor pagination metadata; cursors are opaque and never parsed by clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CursorPayload {
    created_at: DateTime<Utc>,
    id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    pub limit: i64,
}

impl Pagination {
    pub fn from_query(limit: Option<i64>) -> Self {
        Self {
            limit: limit.unwrap_or(50).clamp(1, 100),
        }
    }
}

pub fn encode_cursor(created_at: DateTime<Utc>, id: Uuid) -> String {
    let payload = CursorPayload { created_at, id };
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn decode_cursor(raw: &str) -> Option<(DateTime<Utc>, Uuid)> {
    if raw.is_empty() || raw.len() > 512 {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(raw.as_bytes()).ok()?;
    let payload: CursorPayload = serde_json::from_slice(&bytes).ok()?;
    Some((payload.created_at, payload.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip() {
        let at = Utc::now();
        let id = Uuid::new_v4();
        let encoded = encode_cursor(at, id);
        let (decoded_at, decoded_id) = decode_cursor(&encoded).unwrap();
        assert_eq!(decoded_id, id);
        assert_eq!(decoded_at.timestamp(), at.timestamp());
        assert!(decode_cursor("%%%").is_none());
    }
}
