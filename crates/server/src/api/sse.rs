use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Versioned SSE envelope. Sequence is monotonic per stream and supports reconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SseEnvelope {
    pub version: u16,
    pub sequence: u64,
    pub event: String,
    pub request_id: String,
    pub data: Value,
}
