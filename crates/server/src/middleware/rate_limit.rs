use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::header::{AUTHORIZATION, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use uuid::Uuid;

use crate::api::ApiError;
use crate::config::RateLimitConfig;
use crate::http::AppState;
use crate::middleware::request_id::{RequestId, X_REQUEST_ID};

const WINDOW: Duration = Duration::from_secs(60);
const IDLE_TTL: Duration = Duration::from_secs(10 * 60);
const FALLBACK_PEER: &str = "unknown-peer";

#[derive(Debug)]
pub(crate) struct RateLimiter {
    buckets: Mutex<RateLimitStore>,
    max_entries: usize,
}

#[derive(Debug)]
struct RateLimitStore {
    buckets: HashMap<BucketKey, Bucket>,
    last_eviction: tokio::time::Instant,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BucketKey {
    class: RateClass,
    identity: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RateClass {
    Auth,
    Llm,
    Authenticated,
    Fallback,
}

#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    refilled_at: tokio::time::Instant,
    last_seen: tokio::time::Instant,
}

#[derive(Debug)]
struct RateDecision {
    key: BucketKey,
    limit_per_minute: u32,
}

impl RateLimiter {
    pub(crate) fn new(config: RateLimitConfig) -> Self {
        Self {
            buckets: Mutex::new(RateLimitStore {
                buckets: HashMap::new(),
                last_eviction: tokio::time::Instant::now(),
            }),
            max_entries: config.max_entries,
        }
    }

    fn check(&self, key: BucketKey, limit_per_minute: u32) -> Result<(), Duration> {
        self.check_at(key, limit_per_minute, tokio::time::Instant::now())
    }

    fn check_at(
        &self,
        key: BucketKey,
        limit_per_minute: u32,
        now: tokio::time::Instant,
    ) -> Result<(), Duration> {
        let mut store = self
            .buckets
            .lock()
            .expect("rate limiter mutex must not be poisoned");
        store.evict_if_due(now, self.max_entries);
        if !store.buckets.contains_key(&key) && store.buckets.len() >= self.max_entries {
            store.evict_oldest();
        }

        let capacity = f64::from(limit_per_minute);
        let refill_per_second = capacity / WINDOW.as_secs_f64();
        let bucket = store.buckets.entry(key).or_insert_with(|| Bucket {
            tokens: capacity,
            refilled_at: now,
            last_seen: now,
        });
        let elapsed = now.duration_since(bucket.refilled_at).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_per_second).min(capacity);
        bucket.refilled_at = now;
        bucket.last_seen = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let retry = ((1.0 - bucket.tokens) / refill_per_second).ceil().max(1.0) as u64;
            Err(Duration::from_secs(retry))
        }
    }

    #[cfg(test)]
    fn bucket_count(&self) -> usize {
        self.buckets
            .lock()
            .expect("rate limiter mutex must not be poisoned")
            .buckets
            .len()
    }
}

impl RateLimitStore {
    fn evict_if_due(&mut self, now: tokio::time::Instant, max_entries: usize) {
        if self.buckets.len() < max_entries && now.duration_since(self.last_eviction) < IDLE_TTL {
            return;
        }
        self.last_eviction = now;
        self.buckets
            .retain(|_, bucket| now.duration_since(bucket.last_seen) < IDLE_TTL);
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest) = self
            .buckets
            .iter()
            .min_by_key(|(_, bucket)| bucket.last_seen)
            .map(|(key, _)| key.clone())
        {
            self.buckets.remove(&oldest);
        }
    }
}

pub async fn rate_limit(
    State(state): State<std::sync::Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if is_health_path(request.uri().path()) {
        return next.run(request).await;
    }

    let decision = rate_decision(&state, &request);
    match state
        .rate_limiter()
        .check(decision.key, decision.limit_per_minute)
    {
        Ok(()) => next.run(request).await,
        Err(retry_after) => rate_limited_response(&request, retry_after),
    }
}

fn is_health_path(path: &str) -> bool {
    path.starts_with("/api/v1/health/")
}

fn rate_decision(state: &AppState, request: &Request<Body>) -> RateDecision {
    let config = state.runtime().config().rate_limits();
    let path = request.uri().path();
    let user_key = verified_user_key(state, request.headers());
    if matches!(path, "/api/v1/auth/login" | "/api/v1/auth/refresh") {
        return RateDecision {
            key: BucketKey {
                class: RateClass::Auth,
                identity: ip_key(state, request),
            },
            limit_per_minute: config.auth_per_ip_per_minute,
        };
    }
    if matches!(
        path,
        "/api/v1/search" | "/api/v1/ask" | "/api/v1/ask/stream"
    ) {
        if let Some(identity) = user_key {
            return RateDecision {
                key: BucketKey {
                    class: RateClass::Llm,
                    identity,
                },
                limit_per_minute: config.llm_per_user_per_minute,
            };
        }
        return fallback_decision(state, request, config);
    }
    if let Some(identity) = user_key {
        RateDecision {
            key: BucketKey {
                class: RateClass::Authenticated,
                identity,
            },
            limit_per_minute: config.authenticated_per_user_per_minute,
        }
    } else {
        fallback_decision(state, request, config)
    }
}

fn fallback_decision(
    state: &AppState,
    request: &Request<Body>,
    config: RateLimitConfig,
) -> RateDecision {
    RateDecision {
        key: BucketKey {
            class: RateClass::Fallback,
            identity: ip_key(state, request),
        },
        limit_per_minute: config.fallback_per_ip_per_minute,
    }
}

fn verified_user_key(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let token = bearer_token(headers)?;
    let claims = state
        .auth_provider()
        .and_then(|provider| provider.keys().verify_access_token(token).ok())?;
    Some(format!("org:{}:user:{}", claims.org_id, claims.sub))
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))?;
    (!token.is_empty() && token.len() <= 4096).then_some(token)
}

fn ip_key(state: &AppState, request: &Request<Body>) -> String {
    if state.runtime().config().trusted_proxy() {
        if let Some(ip) = forwarded_ip(request.headers()) {
            return format!("ip:{ip}");
        }
    }
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| format!("ip:{}", addr.ip()))
        .unwrap_or_else(|| format!("ip:{FALLBACK_PEER}"))
}

fn forwarded_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .and_then(|value| value.parse::<IpAddr>().ok())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<IpAddr>().ok())
        })
}

fn rate_limited_response(request: &Request<Body>, retry_after: Duration) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|request_id| request_id.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let retry_seconds = retry_after.as_secs().max(1).to_string();
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(ApiError {
            code: "rate_limited".into(),
            message: "Too many requests; please retry later".into(),
            request_id: request_id.clone(),
            details: None,
        }),
    )
        .into_response();
    response.headers_mut().insert(
        RETRY_AFTER,
        HeaderValue::from_str(&retry_seconds).expect("retry seconds are a valid header value"),
    );
    response.headers_mut().insert(
        X_REQUEST_ID.clone(),
        HeaderValue::from_str(&request_id).expect("UUID is a valid header value"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(max_entries: usize) -> RateLimitConfig {
        RateLimitConfig {
            auth_per_ip_per_minute: 2,
            llm_per_user_per_minute: 2,
            authenticated_per_user_per_minute: 2,
            fallback_per_ip_per_minute: 2,
            max_entries,
        }
    }

    #[test]
    fn token_bucket_refills_and_reports_retry_after() {
        let limiter = RateLimiter::new(test_config(16));
        let key = BucketKey {
            class: RateClass::Fallback,
            identity: "ip:127.0.0.1".into(),
        };
        let now = tokio::time::Instant::now();
        assert!(limiter.check_at(key.clone(), 2, now).is_ok());
        assert!(limiter.check_at(key.clone(), 2, now).is_ok());
        assert_eq!(
            limiter.check_at(key.clone(), 2, now).unwrap_err(),
            Duration::from_secs(30)
        );
        assert!(limiter
            .check_at(key, 2, now + Duration::from_secs(30))
            .is_ok());
    }

    #[test]
    fn bucket_map_is_capped() {
        let limiter = RateLimiter::new(test_config(2));
        let now = tokio::time::Instant::now();
        for index in 0..10 {
            let key = BucketKey {
                class: RateClass::Fallback,
                identity: format!("ip:192.0.2.{index}"),
            };
            assert!(limiter.check_at(key, 2, now).is_ok());
        }
        assert_eq!(limiter.bucket_count(), 2);
    }
}
