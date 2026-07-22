//! Bounded page size and opaque keyset cursors for `/api/v1` list routes.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error::ApiRejection;

/// Inclusive lower bound for `page` / `limit` query parameters.
pub const MIN_PAGE_SIZE: u32 = 1;
/// Inclusive upper bound for `page` / `limit` query parameters.
pub const MAX_PAGE_SIZE: u32 = 100;
/// Default page size when the client omits `limit`.
pub const DEFAULT_PAGE_SIZE: u32 = 20;

/// Cursor pagination metadata; cursors are opaque and never parsed by clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

impl PageInfo {
    pub fn end() -> Self {
        Self {
            next_cursor: None,
            has_more: false,
        }
    }

    pub fn more(next_cursor: impl Into<String>) -> Self {
        Self {
            next_cursor: Some(next_cursor.into()),
            has_more: true,
        }
    }
}

/// Parsed list paging controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageParams {
    pub limit: u32,
    pub cursor: Option<String>,
}

impl PageParams {
    pub fn new(limit: u32, cursor: Option<String>) -> Result<Self, String> {
        if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(format!(
                "limit must be between {MIN_PAGE_SIZE} and {MAX_PAGE_SIZE}"
            ));
        }
        if let Some(ref value) = cursor {
            if value.is_empty() || value.len() > 512 {
                return Err("cursor must be an opaque token".into());
            }
            if !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err("cursor must be an opaque token".into());
            }
        }
        Ok(Self { limit, cursor })
    }

    pub fn from_query(
        limit: Option<u32>,
        cursor: Option<String>,
        request_id: &str,
    ) -> Result<Self, ApiRejection> {
        let resolved_limit = limit.unwrap_or(DEFAULT_PAGE_SIZE);
        if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&resolved_limit) {
            return Err(ApiRejection::validation(
                format!("limit must be between {MIN_PAGE_SIZE} and {MAX_PAGE_SIZE}"),
                request_id,
            )
            .with_details(serde_json::json!({ "field": "limit" })));
        }
        if let Some(ref value) = cursor {
            if value.is_empty() || value.len() > 512 {
                return Err(
                    ApiRejection::validation("cursor must be an opaque token", request_id)
                        .with_details(serde_json::json!({ "field": "cursor" })),
                );
            }
            if !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(
                    ApiRejection::validation("cursor must be an opaque token", request_id)
                        .with_details(serde_json::json!({ "field": "cursor" })),
                );
            }
        }
        Ok(Self {
            limit: resolved_limit,
            cursor,
        })
    }
}

/// Keyset cursor for `(created_at, id)` style lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedAtIdCursor {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub id: Uuid,
}

/// Keyset cursor for `(name, id)` collection lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameIdCursor {
    pub name: String,
    pub id: Uuid,
}

/// Keyset cursor for `(version_number, id)` version lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionNumberIdCursor {
    pub version_number: i32,
    pub id: Uuid,
}

pub fn encode_cursor<T: Serialize>(value: &T) -> Result<String, String> {
    let json = serde_json::to_vec(value).map_err(|_| "cursor encoding failed".to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

pub fn decode_cursor<T: for<'de> Deserialize<'de>>(raw: &str) -> Result<T, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .map_err(|_| "cursor must be an opaque token".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "cursor must be an opaque token".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_size_is_bounded() {
        assert!(PageParams::new(0, None).is_err());
        assert!(PageParams::new(101, None).is_err());
        assert_eq!(PageParams::new(20, None).unwrap().limit, 20);
    }

    #[test]
    fn cursor_round_trips() {
        let cursor = CreatedAtIdCursor {
            created_at: chrono::Utc::now(),
            id: Uuid::new_v4(),
        };
        let encoded = encode_cursor(&cursor).unwrap();
        let decoded: CreatedAtIdCursor = decode_cursor(&encoded).unwrap();
        assert_eq!(decoded.id, cursor.id);
    }

    #[test]
    fn malformed_cursor_is_rejected() {
        assert!(PageParams::new(10, Some("!!!".into())).is_err());
        assert!(decode_cursor::<CreatedAtIdCursor>("not-a-cursor").is_err());
    }

    #[test]
    fn from_query_separates_limit_and_cursor_error_fields() {
        let limit_err = PageParams::from_query(Some(0), None, "req-1").unwrap_err();
        let limit_body = limit_err.body().clone();
        assert_eq!(limit_body.details.unwrap()["field"], "limit");

        let cursor_err = PageParams::from_query(Some(10), Some("!!!".into()), "req-2").unwrap_err();
        let cursor_body = cursor_err.body().clone();
        assert_eq!(cursor_body.details.unwrap()["field"], "cursor");

        // Both present: limit is validated first.
        let both = PageParams::from_query(Some(0), Some("!!!".into()), "req-3").unwrap_err();
        assert_eq!(both.body().details.as_ref().unwrap()["field"], "limit");
    }
}
