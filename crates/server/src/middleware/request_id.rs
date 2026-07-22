//! Validate / generate / echo `X-Request-Id` for every request.

use std::convert::Infallible;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// Canonical correlation header (validated UUID only).
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Server-accepted request identifier stashed in request extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

impl RequestId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Infallible extractor that prefers middleware `RequestId`, else mints one.
#[derive(Debug, Clone)]
pub struct ResolvedRequestId(pub String);

impl ResolvedRequestId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<S> FromRequestParts<S> for ResolvedRequestId
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let id = parts
            .extensions
            .get::<RequestId>()
            .map(|id| id.0.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        Ok(Self(id))
    }
}

/// Accept only canonical UUID strings (rejects opaque/secret-like values).
pub fn validate_request_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() > 36 {
        return None;
    }
    Uuid::parse_str(trimmed).ok()?;
    // Normalise to hyphenated lowercase form.
    Some(Uuid::parse_str(trimmed).ok()?.to_string())
}

pub fn resolve_or_generate(header_value: Option<&str>) -> String {
    header_value
        .and_then(validate_request_id)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let incoming = request
        .headers()
        .get(&REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok());
    let request_id = resolve_or_generate(incoming);
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{resolve_or_generate, validate_request_id};

    #[test]
    fn accepts_canonical_uuid_and_rejects_garbage() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(validate_request_id(id).as_deref(), Some(id));
        assert!(validate_request_id("not-a-uuid").is_none());
        assert!(validate_request_id("refresh-token-looking-value").is_none());
        assert!(validate_request_id(&"a".repeat(64)).is_none());
    }

    #[test]
    fn generates_when_missing_or_invalid() {
        let generated = resolve_or_generate(None);
        assert!(validate_request_id(&generated).is_some());
        let regenerated = resolve_or_generate(Some("bad"));
        assert_ne!(regenerated, "bad");
        assert!(validate_request_id(&regenerated).is_some());
    }
}
