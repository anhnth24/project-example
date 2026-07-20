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
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);
const FALLBACK_PEER: &str = "unknown-peer";

#[derive(Debug)]
pub(crate) struct RateLimiter {
    buckets: Mutex<RateLimitStore>,
    max_entries: usize,
}

#[derive(Debug)]
struct RateLimitStore {
    buckets: HashMap<BucketKey, Bucket>,
    last_sweep: tokio::time::Instant,
    #[cfg(test)]
    sweep_count: usize,
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
                last_sweep: tokio::time::Instant::now(),
                #[cfg(test)]
                sweep_count: 0,
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
        let mut store = self.lock_store();
        store.sweep_expired_if_due(now);
        if let Some(bucket) = store.buckets.get_mut(&key) {
            return check_bucket(bucket, limit_per_minute, now);
        }

        if store.buckets.len() >= self.max_entries {
            // Fail open at hard capacity. The table is already memory-bounded, and
            // scanning for an eviction victim here would make novel attacker keys
            // impose O(N) work under the global mutex.
            return Ok(());
        }
        let capacity = f64::from(limit_per_minute);
        store.buckets.insert(
            key,
            Bucket {
                tokens: capacity - 1.0,
                refilled_at: now,
                last_seen: now,
            },
        );
        Ok(())
    }

    fn lock_store(&self) -> std::sync::MutexGuard<'_, RateLimitStore> {
        self.buckets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    fn bucket_count(&self) -> usize {
        self.lock_store().buckets.len()
    }

    #[cfg(test)]
    fn sweep_count(&self) -> usize {
        self.lock_store().sweep_count
    }
}

fn check_bucket(
    bucket: &mut Bucket,
    limit_per_minute: u32,
    now: tokio::time::Instant,
) -> Result<(), Duration> {
    let capacity = f64::from(limit_per_minute);
    let refill_per_second = capacity / WINDOW.as_secs_f64();
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

impl RateLimitStore {
    fn sweep_expired_if_due(&mut self, now: tokio::time::Instant) {
        if now.duration_since(self.last_sweep) < SWEEP_INTERVAL {
            return;
        }
        self.last_sweep = now;
        #[cfg(test)]
        {
            self.sweep_count += 1;
        }
        self.buckets
            .retain(|_, bucket| now.duration_since(bucket.last_seen) <= WINDOW);
    }
}

pub async fn rate_limit(
    State(state): State<std::sync::Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if is_exempt_path(request.uri().path()) {
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

fn is_exempt_path(path: &str) -> bool {
    matches!(
        path,
        "/api/v1/health/live" | "/api/v1/health/ready" | "/api/v1/health/start" | "/api/v1/metrics"
    )
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

    #[test]
    fn saturated_map_fails_open_for_novel_keys_without_losing_existing_limits() {
        let limiter = RateLimiter::new(test_config(2));
        let now = tokio::time::Instant::now();
        let first = BucketKey {
            class: RateClass::Fallback,
            identity: "ip:192.0.2.1".into(),
        };
        let second = BucketKey {
            class: RateClass::Fallback,
            identity: "ip:192.0.2.2".into(),
        };
        let novel = BucketKey {
            class: RateClass::Fallback,
            identity: "ip:192.0.2.3".into(),
        };

        assert!(limiter.check_at(first.clone(), 1, now).is_ok());
        assert!(limiter.check_at(second, 1, now).is_ok());
        assert!(limiter.check_at(novel, 1, now).is_ok());
        assert_eq!(limiter.bucket_count(), 2);
        assert_eq!(
            limiter.check_at(first, 1, now).unwrap_err(),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn flood_is_bounded_and_sweeps_only_on_interval() {
        let limiter = RateLimiter::new(test_config(4));
        let now = tokio::time::Instant::now();
        for index in 0..100 {
            let key = BucketKey {
                class: RateClass::Fallback,
                identity: format!("ip:198.51.100.{index}"),
            };
            assert!(limiter.check_at(key, 2, now).is_ok());
        }
        assert_eq!(limiter.bucket_count(), 4);
        assert_eq!(limiter.sweep_count(), 0);

        let before_sweep = BucketKey {
            class: RateClass::Fallback,
            identity: "ip:203.0.113.1".into(),
        };
        assert!(limiter
            .check_at(
                before_sweep,
                2,
                now + SWEEP_INTERVAL - Duration::from_secs(1)
            )
            .is_ok());
        assert_eq!(limiter.bucket_count(), 4);
        assert_eq!(limiter.sweep_count(), 0);

        let first_due = BucketKey {
            class: RateClass::Fallback,
            identity: "ip:203.0.113.2".into(),
        };
        assert!(limiter.check_at(first_due, 2, now + SWEEP_INTERVAL).is_ok());
        assert_eq!(limiter.bucket_count(), 4);
        assert_eq!(limiter.sweep_count(), 1);

        let after_expiry = BucketKey {
            class: RateClass::Fallback,
            identity: "ip:203.0.113.3".into(),
        };
        assert!(limiter
            .check_at(
                after_expiry,
                2,
                now + WINDOW + SWEEP_INTERVAL + Duration::from_secs(1),
            )
            .is_ok());
        assert_eq!(limiter.bucket_count(), 1);
        assert_eq!(limiter.sweep_count(), 2);
    }

    #[test]
    fn poisoned_mutex_recovers_for_future_requests() {
        let limiter = RateLimiter::new(test_config(2));
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = limiter.lock_store();
            panic!("intentional poison");
        }));
        assert!(poisoned.is_err());

        let key = BucketKey {
            class: RateClass::Fallback,
            identity: "ip:192.0.2.44".into(),
        };
        assert!(limiter
            .check_at(key, 2, tokio::time::Instant::now())
            .is_ok());
    }
}
