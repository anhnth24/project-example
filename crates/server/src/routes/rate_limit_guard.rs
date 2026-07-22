//! Shared 429 responses for in-process rate limiting (P1B-R06).

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::ApiError;
use crate::http::AppState;

#[derive(Debug, Clone)]
pub struct RateLimitRejected {
    pub retry_after: Duration,
    pub request_id: String,
}

impl IntoResponse for RateLimitRejected {
    fn into_response(self) -> Response {
        let secs = self.retry_after.as_secs().max(1);
        let retry_header = axum::http::HeaderValue::from_str(&secs.to_string())
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("1"));
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(axum::http::header::RETRY_AFTER, retry_header)],
            Json(ApiError {
                code: "rate_limited".into(),
                message: "Too many requests".into(),
                request_id: self.request_id,
                details: Some(serde_json::json!({ "retryAfterSeconds": secs })),
            }),
        )
            .into_response()
    }
}

pub fn reject_or_allow(
    check: Result<(), Duration>,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    match check {
        Ok(()) => Ok(()),
        Err(retry_after) => Err(RateLimitRejected {
            retry_after,
            request_id: request_id.to_string(),
        }),
    }
}

pub fn check_ip(
    state: &Arc<AppState>,
    ip: &str,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    reject_or_allow(state.rate_limiter().check_ip(ip), request_id)
}

pub fn check_auth_ip(
    state: &Arc<AppState>,
    ip: &str,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    reject_or_allow(state.rate_limiter().check_auth_ip(ip), request_id)
}

pub fn check_user(
    state: &Arc<AppState>,
    org_id: &str,
    user_id: &str,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    reject_or_allow(state.rate_limiter().check_user(org_id, user_id), request_id)
}

pub fn check_route(
    state: &Arc<AppState>,
    route: &str,
    ip: &str,
    request_id: &str,
) -> Result<(), RateLimitRejected> {
    reject_or_allow(state.rate_limiter().check_route(route, ip), request_id)
}
