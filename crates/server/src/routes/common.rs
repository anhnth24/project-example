use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::{ApiError, PageInfo};
use crate::auth::context::OrgContext;
use crate::auth::permissions::{require_collection, require_permission};
use crate::db::error::DbError;
use crate::services::download::CapabilityKey;

const DEFAULT_LIMIT: usize = 25;
const MAX_LIMIT: usize = 100;
const CURSOR_DOMAIN: &[u8] = b"markhand-rest-cursor-v1";

#[derive(Debug)]
pub(crate) struct RestError {
    status: StatusCode,
    code: &'static str,
    message: String,
    request_id: String,
}

impl RestError {
    pub(crate) fn validation(message: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "validation_failed",
            message: message.into(),
            request_id: request_id.into(),
        }
    }

    pub(crate) fn forbidden(request_id: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "permission_denied",
            message: "Permission denied".into(),
            request_id: request_id.into(),
        }
    }

    pub(crate) fn empty_scope(request_id: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "empty_scope",
            message: "No authorized collections are available".into(),
            request_id: request_id.into(),
        }
    }

    pub(crate) fn not_found(request_id: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "Resource was not found".into(),
            request_id: request_id.into(),
        }
    }

    pub(crate) fn conflict(message: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "state_conflict",
            message: message.into(),
            request_id: request_id.into(),
        }
    }

    pub(crate) fn internal(request_id: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "Request failed".into(),
            request_id: request_id.into(),
        }
    }

    pub(crate) fn service_unavailable(request_id: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "dependency_unavailable",
            message: "A required service is unavailable".into(),
            request_id: request_id.into(),
        }
    }

    pub(crate) fn too_many_requests(
        message: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: message.into(),
            request_id: request_id.into(),
        }
    }
}

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiError {
                code: self.code.into(),
                message: self.message,
                request_id: self.request_id,
                details: None,
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PageParams {
    pub(crate) limit: Option<String>,
    pub(crate) cursor: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PageLimit {
    pub(crate) page_size: usize,
    pub(crate) fetch_size: i64,
}

pub(crate) fn parse_page_limit(
    params: &PageParams,
    request_id: &str,
) -> Result<PageLimit, RestError> {
    let page_size = match params.limit.as_deref() {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| RestError::validation("limit must be an integer", request_id))?,
        None => DEFAULT_LIMIT,
    };
    if !(1..=MAX_LIMIT).contains(&page_size) {
        return Err(RestError::validation(
            "limit must be between 1 and 100",
            request_id,
        ));
    }
    Ok(PageLimit {
        page_size,
        fetch_size: (page_size + 1) as i64,
    })
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorPayload<T> {
    kind: String,
    value: T,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorEnvelope<T> {
    payload: CursorPayload<T>,
    tag: String,
}

pub(crate) fn encode_cursor<T: Serialize>(
    key: &CapabilityKey,
    kind: &str,
    value: &T,
) -> Result<String, RestError> {
    let payload = CursorPayload {
        kind: kind.to_string(),
        value,
    };
    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|_| RestError::internal(Uuid::new_v4().to_string()))?;
    let tag = URL_SAFE_NO_PAD.encode(key.sign_domain_separated(CURSOR_DOMAIN, &payload_bytes));
    let envelope = CursorEnvelope { payload, tag };
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|_| RestError::internal(Uuid::new_v4().to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) fn decode_cursor<T: DeserializeOwned + Serialize>(
    key: &CapabilityKey,
    kind: &str,
    cursor: Option<&str>,
    request_id: &str,
) -> Result<Option<T>, RestError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    if cursor.is_empty() || cursor.len() > 512 || cursor.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RestError::validation("cursor is invalid", request_id));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| RestError::validation("cursor is invalid", request_id))?;
    let envelope: CursorEnvelope<T> = serde_json::from_slice(&bytes)
        .map_err(|_| RestError::validation("cursor is invalid", request_id))?;
    if envelope.payload.kind != kind {
        return Err(RestError::validation("cursor is invalid", request_id));
    }
    let payload_bytes = serde_json::to_vec(&envelope.payload)
        .map_err(|_| RestError::validation("cursor is invalid", request_id))?;
    let tag = URL_SAFE_NO_PAD
        .decode(envelope.tag)
        .map_err(|_| RestError::validation("cursor is invalid", request_id))?;
    if !key.verify_domain_separated(CURSOR_DOMAIN, &payload_bytes, &tag) {
        return Err(RestError::validation("cursor is invalid", request_id));
    }
    Ok(Some(envelope.payload.value))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListResponse<T> {
    pub(crate) items: Vec<T>,
    pub(crate) page_info: PageInfo,
}

pub(crate) fn page_info(next_cursor: Option<String>) -> PageInfo {
    PageInfo {
        has_more: next_cursor.is_some(),
        next_cursor,
    }
}

pub(crate) fn parse_uuid(value: &str, request_id: &str) -> Result<Uuid, RestError> {
    Uuid::parse_str(value)
        .map_err(|_| RestError::validation("UUID path parameter is invalid", request_id))
}

pub(crate) fn require_permission_or_403(
    ctx: &OrgContext,
    permission: &str,
    request_id: &str,
) -> Result<(), RestError> {
    require_permission(ctx, permission).map_err(|_| RestError::forbidden(request_id))
}

pub(crate) fn require_collection_or_404(
    ctx: &OrgContext,
    collection_id: Uuid,
    request_id: &str,
) -> Result<(), RestError> {
    require_collection(ctx, collection_id).map_err(|_| RestError::not_found(request_id))
}

pub(crate) fn db_or_404(error: DbError, request_id: &str) -> RestError {
    match error {
        DbError::NotFound => RestError::not_found(request_id),
        _ => RestError::internal(request_id),
    }
}

pub(crate) enum TxnRestError {
    Db(DbError),
    Rest(RestError),
}

impl From<DbError> for TxnRestError {
    fn from(value: DbError) -> Self {
        Self::Db(value)
    }
}

impl From<RestError> for TxnRestError {
    fn from(value: RestError) -> Self {
        Self::Rest(value)
    }
}

impl TxnRestError {
    pub(crate) fn into_rest(self, request_id: &str) -> RestError {
        match self {
            Self::Db(error) => db_or_404(error, request_id),
            Self::Rest(error) => error,
        }
    }
}

pub(crate) fn validate_idempotency_header(
    headers: &HeaderMap,
    request_id: &str,
) -> Result<Option<String>, RestError> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| RestError::validation("Idempotency-Key must be visible ASCII", request_id))?;
    if value.is_empty() || value.len() > 128 {
        return Err(RestError::validation(
            "Idempotency-Key must be between 1 and 128 bytes",
            request_id,
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(RestError::validation(
            "Idempotency-Key contains unsupported characters",
            request_id,
        ));
    }
    Ok(Some(value.to_string()))
}
