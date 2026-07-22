//! Validate / generate / echo `X-Request-Id` for every request.

use std::convert::Infallible;
use std::time::Instant;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderName, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

use crate::middleware::rate_limit::EndpointClass;
use crate::telemetry::{
    extract_context_from_headers, inject_traceparent_from_span, normalize_http_method,
    normalize_route, record_api_request, scope, CorrelationContext,
};

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

fn route_label(method: &Method, path: &str) -> &'static str {
    // Low-cardinality route class (never raw path/query).
    let class = match EndpointClass::classify(method, path) {
        EndpointClass::Ready | EndpointClass::LiveStartup => "health",
        EndpointClass::Auth => "auth",
        EndpointClass::Upload => "upload",
        EndpointClass::Search => "search",
        EndpointClass::Stream => "stream",
        EndpointClass::Default => {
            if path.starts_with("/api/v1/documents") {
                "documents"
            } else if path.starts_with("/api/v1/collections") {
                "collections"
            } else if path.starts_with("/api/v1/jobs") {
                "jobs"
            } else if path.starts_with("/api/v1/openapi") {
                "openapi"
            } else {
                "other"
            }
        }
    };
    normalize_route(class)
}

pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    let incoming = request
        .headers()
        .get(&REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok());
    let request_id = resolve_or_generate(incoming);
    // Bound method for metrics/logs; custom/canary methods become OTHER and are never logged raw.
    let method_label = normalize_http_method(request.method().as_str());
    let route = route_label(request.method(), request.uri().path()).to_string();
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    let parent_cx = extract_context_from_headers(
        request
            .headers()
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str(), v))),
    );

    let mut correlation = CorrelationContext::new(request_id.clone());
    let started = Instant::now();
    let span = tracing::info_span!(
        "http_request",
        request_id = %correlation.request_id,
        route = %route,
        method = %method_label,
    );
    let _ = span.set_parent(parent_cx);
    if let Some(traceparent) = inject_traceparent_from_span(&span) {
        correlation.traceparent = Some(traceparent.clone());
        if let Some(trace_id) =
            crate::telemetry::correlation::trace_id_from_traceparent(&traceparent)
        {
            correlation.trace_id = trace_id;
        }
    }

    let response = scope(correlation, next.run(request)).instrument(span).await;

    let status = response.status().as_u16();
    record_api_request(&route, method_label, status, started.elapsed());

    let mut response = response;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::{resolve_or_generate, route_label, validate_request_id};
    use crate::telemetry::normalize_http_method;
    use axum::http::Method;

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

    #[test]
    fn route_labels_are_low_cardinality() {
        assert_eq!(route_label(&Method::GET, "/api/v1/health/live"), "health");
        assert_eq!(
            route_label(
                &Method::GET,
                "/api/v1/documents/550e8400-e29b-41d4-a716-446655440000"
            ),
            "documents"
        );
        assert_ne!(
            route_label(
                &Method::GET,
                "/api/v1/documents/550e8400-e29b-41d4-a716-446655440000"
            ),
            "/api/v1/documents/550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn canary_custom_method_never_kept_as_label() {
        assert_eq!(normalize_http_method("CANARY_CUSTOM_METHOD"), "OTHER");
        assert_eq!(normalize_http_method("PROPFIND"), "OTHER");
    }
}
