//! Versioned wire-contract types shared by future routes, workers and fixtures.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical error body returned by `/api/v1` endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Cursor pagination metadata; cursors are opaque and never parsed by clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

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

#[cfg(test)]
mod tests {
    use super::{ApiError, PageInfo, SseEnvelope};

    #[test]
    fn fixtures_round_trip_through_wire_types() {
        let error: ApiError =
            serde_json::from_str(include_str!("../openapi/fixtures/error.json")).unwrap();
        assert_eq!(error.code, "validation_failed");
        assert_eq!(
            serde_json::to_string_pretty(&error).unwrap(),
            include_str!("../openapi/fixtures/error.json").trim()
        );

        let page: PageInfo =
            serde_json::from_str(include_str!("../openapi/fixtures/pagination.json")).unwrap();
        assert!(page.has_more);

        let event: SseEnvelope =
            serde_json::from_str(include_str!("../openapi/fixtures/sse.json")).unwrap();
        assert_eq!(event.version, 1);
        assert_eq!(event.sequence, 42);
    }
}
