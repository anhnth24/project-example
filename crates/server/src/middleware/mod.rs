//! HTTP middleware: rate limits, CORS, request IDs, mutation write-gate (P1B-R06/O03).

pub mod rate_limit;
pub mod write_gate;

pub use write_gate::{
    ensure_background_mutations_allowed, is_write_gate_exempt, mutation_write_gate,
    BACKUP_ADVISORY_LOCK_KEY, WRITE_GATE_CONTRACT_ID,
};

use std::net::IpAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;
use uuid::Uuid;

use crate::http::AppState;
use crate::routes::rate_limit_guard::RateLimitRejected;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Resolved client IP for rate limiting (never trusts unauthenticated XFF).
#[derive(Debug, Clone)]
pub struct ClientIp(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientIpError {
    /// Peer is a trusted proxy but XFF is missing/invalid — fail closed.
    InvalidForwardedFor,
}

/// Resolves client IP for rate limiting.
///
/// - Peer not in trusted proxies → use peer (ignore XFF).
/// - Peer is trusted → parse XFF right-to-left and pick the first hop that is
///   not itself a trusted proxy. Missing/invalid XFF fails fast.
pub fn resolve_client_ip<B>(
    request: &Request<B>,
    trusted_proxies: &[IpAddr],
) -> Result<String, ClientIpError> {
    let peer = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip());
    let Some(peer_ip) = peer else {
        return Ok("unknown".into());
    };
    if !trusted_proxies.iter().any(|proxy| proxy == &peer_ip) {
        return Ok(peer_ip.to_string());
    }
    let forwarded = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .ok_or(ClientIpError::InvalidForwardedFor)?;
    client_ip_from_xff(forwarded, trusted_proxies)
}

/// Right-to-left XFF walk: prefer the rightmost address that is not trusted.
pub fn client_ip_from_xff(
    forwarded: &str,
    trusted_proxies: &[IpAddr],
) -> Result<String, ClientIpError> {
    let mut chosen: Option<IpAddr> = None;
    for part in forwarded.split(',').rev() {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed.len() > 64 {
            return Err(ClientIpError::InvalidForwardedFor);
        }
        let Ok(ip) = trimmed.parse::<IpAddr>() else {
            return Err(ClientIpError::InvalidForwardedFor);
        };
        if trusted_proxies.iter().any(|proxy| proxy == &ip) {
            continue;
        }
        chosen = Some(ip);
        break;
    }
    chosen
        .map(|ip| ip.to_string())
        .ok_or(ClientIpError::InvalidForwardedFor)
}

/// Backward-compatible helper used by tests/callers that ignore fail-fast.
pub fn client_ip<B>(request: &Request<B>, trusted_proxies: &[IpAddr]) -> String {
    resolve_client_ip(request, trusted_proxies).unwrap_or_else(|_| "unknown".into())
}

pub fn state_limiter(state: &Arc<AppState>) -> &rate_limit::RateLimiter {
    state.rate_limiter()
}

/// Optional client-supplied correlation hint (never used as durable audit request id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRequestId(pub String);

pub async fn inject_request_id(
    state: axum::extract::State<Arc<AppState>>,
    mut request: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Always mint the internal request id. Client X-Request-Id is advisory only.
    let request_id = Uuid::new_v4().to_string();
    if let Some(client_id) = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(validate_request_id)
    {
        request.extensions_mut().insert(ClientRequestId(client_id));
    }
    let inbound_traceparent = request
        .headers()
        .get("traceparent")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| crate::telemetry::validate_traceparent(value).is_ok())
        .map(|value| value.to_ascii_lowercase());
    // Parent context from inbound traceparent; child span id is minted per request.
    let correlation = if let Some(ref tp) = inbound_traceparent {
        crate::telemetry::CorrelationContext::child_of(request_id.clone(), tp)
            .unwrap_or_else(|| crate::telemetry::CorrelationContext::new(request_id.clone()))
    } else {
        crate::telemetry::CorrelationContext::new(request_id.clone())
    };
    let ip = match resolve_client_ip(&request, state.trusted_proxies()) {
        Ok(ip) => ip,
        Err(ClientIpError::InvalidForwardedFor) => {
            let mut response = Response::new(Body::from(
                serde_json::json!({
                    "code": "invalid_forwarded_for",
                    "message": "Trusted proxy forwarded an invalid X-Forwarded-For chain",
                    "requestId": request_id,
                })
                .to_string(),
            ));
            *response.status_mut() = StatusCode::BAD_REQUEST;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            if let Ok(value) = HeaderValue::from_str(&request_id) {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
            }
            return response;
        }
    };
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    request.extensions_mut().insert(ClientIp(ip));
    request.extensions_mut().insert(correlation.clone());
    let route = classify_route(request.uri().path());
    let started = std::time::Instant::now();
    let response = crate::telemetry::scope(correlation.clone(), async move {
        let mut response = next.run(request).await;
        let status_class = match response.status().as_u16() {
            100..=199 => "1xx",
            200..=299 => "2xx",
            300..=399 => "3xx",
            400..=499 => "4xx",
            _ => "5xx",
        };
        let elapsed = started.elapsed();
        crate::telemetry::record_http_request(&route, status_class, elapsed);
        // Export the correlation span id as the SERVER root (children parent to it).
        crate::telemetry::complete_current_span("api.request", "SERVER", status_class, elapsed);
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            response
                .headers_mut()
                .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
        }
        if let Some(tp) = correlation.traceparent.as_deref() {
            if let Ok(value) = HeaderValue::from_str(tp) {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static("traceparent"), value);
            }
        }
        response
    })
    .await;
    response
}

/// Accept only canonical UUID strings (rejects opaque/secret-like values).
pub fn validate_request_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() > 36 {
        return None;
    }
    Uuid::parse_str(trimmed).ok().map(|id| id.to_string())
}

fn classify_route(path: &str) -> String {
    // Keep cardinality bounded: collapse UUID/id segments.
    let mut parts = Vec::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if looks_like_id(segment) {
            parts.push("{id}");
        } else {
            parts.push(segment);
        }
    }
    if parts.is_empty() {
        "root".into()
    } else {
        parts.join(".")
    }
}

fn looks_like_id(segment: &str) -> bool {
    if segment.len() >= 32 && segment.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return true;
    }
    uuid::Uuid::parse_str(segment).is_ok()
}

/// Baseline IP rate limit for `/api/v1/*` except health probes.
pub async fn baseline_ip_rate_limit(
    state: axum::extract::State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let path = request.uri().path();
    if path.starts_with("/api/v1/health/") || path == "/metrics" {
        return next.run(request).await;
    }
    if !path.starts_with("/api/v1/") {
        return next.run(request).await;
    }
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| "missing-middleware-request-id".into());
    let ip = request
        .extensions()
        .get::<ClientIp>()
        .map(|ip| ip.0.clone())
        .unwrap_or_else(|| "unknown".into());
    if let Err(retry_after) = state.rate_limiter().check_ip(&ip) {
        // Shared ceil Retry-After + body/header/quota metadata with route guards.
        return RateLimitRejected {
            retry_after,
            request_id,
            scope: "ip",
        }
        .into_response();
    }
    next.run(request).await
}

/// Conservative CORS: reflect allowlisted Origin only; deny credentials otherwise.
pub async fn cors_middleware(
    state: axum::extract::State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let allowed = match (&origin, state.cors_origins()) {
        (Some(origin), origins) if origins.iter().any(|item| item == origin) => {
            Some(origin.clone())
        }
        (None, _) => None,
        _ => None,
    };

    if request.method() == Method::OPTIONS {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::NO_CONTENT;
        apply_cors_headers(&mut response, allowed.as_deref());
        return response;
    }

    let mut response = next.run(request).await;
    apply_cors_headers(&mut response, allowed.as_deref());
    response
}

fn apply_cors_headers(response: &mut Response<Body>, origin: Option<&str>) {
    let Some(origin) = origin else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(origin) {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization, content-type, idempotency-key, last-event-id"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"),
    );
    response
        .headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Origin"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn xff_walks_right_to_left_skipping_trusted() {
        let trusted = [IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))];
        let ip = client_ip_from_xff("203.0.113.9, 10.0.0.1", &trusted).unwrap();
        assert_eq!(ip, "203.0.113.9");
        let ip = client_ip_from_xff("198.51.100.2, 203.0.113.9, 10.0.0.1", &trusted).unwrap();
        assert_eq!(ip, "203.0.113.9");
    }

    #[test]
    fn xff_fail_fast_on_garbage() {
        let trusted = [IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))];
        assert!(client_ip_from_xff("not-an-ip, 10.0.0.1", &trusted).is_err());
        assert!(client_ip_from_xff("", &trusted).is_err());
        assert!(client_ip_from_xff("10.0.0.1", &trusted).is_err());
    }
}
