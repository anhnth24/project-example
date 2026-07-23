//! Shared 429 responses for in-process rate limiting (P1B-R06).
//!
//! Retry-After is ceiled to whole seconds and mirrored into JSON details so
//! body/header/quota metadata stay consistent under a deterministic clock.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::ApiError;
use crate::http::AppState;

/// Ceil a retry delay to whole seconds (>= 1) for Retry-After + JSON parity.
pub fn ceil_retry_after_secs(retry_after: Duration) -> u64 {
    let secs = retry_after.as_secs();
    let nanos = retry_after.subsec_nanos();
    let ceiled = if nanos > 0 {
        secs.saturating_add(1)
    } else {
        secs
    };
    ceiled.max(1)
}

#[derive(Debug, Clone)]
pub struct RateLimitRejected {
    pub retry_after: Duration,
    pub request_id: String,
    pub scope: &'static str,
}

impl IntoResponse for RateLimitRejected {
    fn into_response(self) -> Response {
        let secs = ceil_retry_after_secs(self.retry_after);
        let retry_header = axum::http::HeaderValue::from_str(&secs.to_string())
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("1"));
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, retry_header)],
            Json(ApiError {
                code: "rate_limited".into(),
                message: "Too many requests".into(),
                request_id: self.request_id,
                details: Some(serde_json::json!({
                    "retryAfterSeconds": secs,
                    "scope": self.scope,
                    "quota": "rate_limit",
                })),
            }),
        )
            .into_response()
    }
}

pub fn reject_or_allow(
    check: Result<(), Duration>,
    request_id: &str,
    scope: &'static str,
) -> Result<(), RateLimitRejected> {
    match check {
        Ok(()) => Ok(()),
        Err(retry_after) => Err(RateLimitRejected {
            retry_after,
            request_id: request_id.to_string(),
            scope,
        }),
    }
}

pub fn check_ip(
    state: &Arc<AppState>,
    ip: &str,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    reject_or_allow(state.rate_limiter().check_ip(ip), request_id, "ip")
}

pub fn check_auth_ip(
    state: &Arc<AppState>,
    ip: &str,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    reject_or_allow(state.rate_limiter().check_auth_ip(ip), request_id, "auth")
}

pub fn check_user(
    state: &Arc<AppState>,
    org_id: &str,
    user_id: &str,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    reject_or_allow(
        state.rate_limiter().check_user(org_id, user_id),
        request_id,
        "user",
    )
}

pub fn check_route(
    state: &Arc<AppState>,
    route: &str,
    ip: &str,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    reject_or_allow(
        state.rate_limiter().check_route(route, ip),
        request_id,
        "route",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceil_retry_after_is_deterministic() {
        assert_eq!(ceil_retry_after_secs(Duration::from_secs(0)), 1);
        assert_eq!(ceil_retry_after_secs(Duration::from_secs(2)), 2);
        assert_eq!(ceil_retry_after_secs(Duration::from_millis(50)), 1);
        assert_eq!(ceil_retry_after_secs(Duration::from_millis(1001)), 2);
        assert_eq!(ceil_retry_after_secs(Duration::from_secs_f64(1.01)), 2);
    }
}
