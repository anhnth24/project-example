//! Bounded in-process fixed-window rate limiter (no distributed claim).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use uuid::Uuid;

use crate::api::ApiError;
use crate::config::RateLimitConfig;
use crate::http::AppState;
use crate::middleware::client_ip::{resolve_client_ip, ClientIpError};
use crate::middleware::request_id::RequestId;

/// Coarse endpoint classes with independent quotas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointClass {
    /// `/ready` and compat — rate limited (not exempt).
    Ready,
    /// `/live` / `/startup` — cheap exempt.
    LiveStartup,
    Auth,
    Upload,
    Search,
    Stream,
    Default,
}

impl EndpointClass {
    pub fn classify(method: &Method, path: &str) -> Self {
        let _ = method;
        if path == "/live"
            || path == "/startup"
            || path == "/api/v1/health/live"
            || path == "/api/v1/health/startup"
        {
            return Self::LiveStartup;
        }
        if path == "/ready" || path == "/api/v1/health/ready" {
            return Self::Ready;
        }
        if path.starts_with("/api/v1/auth/") {
            return Self::Auth;
        }
        if path == "/api/v1/uploads" {
            return Self::Upload;
        }
        if path == "/api/v1/search" || path == "/api/v1/ask" {
            return Self::Search;
        }
        if path == "/api/v1/ask/stream" || path.starts_with("/api/v1/events/") {
            return Self::Stream;
        }
        Self::Default
    }

    pub fn is_cheap_exempt(self) -> bool {
        matches!(self, Self::LiveStartup)
    }

    pub fn is_auth(self) -> bool {
        matches!(self, Self::Auth)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LimitKey {
    Ip {
        class: EndpointClass,
        ip: IpAddr,
    },
    Actor {
        class: EndpointClass,
        org_id: Uuid,
        user_id: Uuid,
    },
}

#[derive(Debug, Clone)]
struct WindowCounter {
    window_start: Instant,
    count: u32,
}

/// Fixed-window counter map with hard cardinality cap.
///
/// Eviction only runs when inserting a **missing** key: prune expired windows first;
/// active counters are never evicted. If the map is still full, the insert is refused.
#[derive(Debug)]
pub struct InMemoryRateLimiter {
    config: RateLimitConfig,
    inner: Mutex<HashMap<LimitKey, WindowCounter>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub limit: u32,
    pub remaining: u32,
    pub reset_secs: u64,
    pub retry_after_secs: u64,
    pub scope: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CheckOutcome {
    Allowed(RateLimitDecision),
    Limited(RateLimitDecision),
    /// New key refused because the map is at capacity after pruning expired entries.
    CardinalityFull,
}

impl InMemoryRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn check_keyed(
        &self,
        key: LimitKey,
        limit: u32,
        scope: &'static str,
        now: Instant,
    ) -> CheckOutcome {
        let window = Duration::from_secs(self.config.window_secs.max(1));
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(entry) = guard.get_mut(&key) {
            if now.duration_since(entry.window_start) >= window {
                entry.window_start = now;
                entry.count = 0;
            }
            return decide(entry, limit, scope, window, now);
        }

        prune_expired(&mut guard, window, now);
        if guard.len() >= self.config.max_keys {
            return CheckOutcome::CardinalityFull;
        }

        let entry = guard.entry(key).or_insert_with(|| WindowCounter {
            window_start: now,
            count: 0,
        });
        decide(entry, limit, scope, window, now)
    }
}

fn decide(
    entry: &mut WindowCounter,
    limit: u32,
    scope: &'static str,
    window: Duration,
    now: Instant,
) -> CheckOutcome {
    let elapsed = now.duration_since(entry.window_start);
    let reset_secs = window.saturating_sub(elapsed).as_secs().max(1);
    if entry.count >= limit {
        return CheckOutcome::Limited(RateLimitDecision {
            allowed: false,
            limit,
            remaining: 0,
            reset_secs,
            retry_after_secs: reset_secs,
            scope,
        });
    }
    entry.count = entry.count.saturating_add(1);
    CheckOutcome::Allowed(RateLimitDecision {
        allowed: true,
        limit,
        remaining: limit.saturating_sub(entry.count),
        reset_secs,
        retry_after_secs: 0,
        scope,
    })
}

fn prune_expired(map: &mut HashMap<LimitKey, WindowCounter>, window: Duration, now: Instant) {
    map.retain(|_, entry| now.duration_since(entry.window_start) < window);
}

fn ip_limit(config: &RateLimitConfig, class: EndpointClass) -> (u32, &'static str) {
    match class {
        EndpointClass::Ready | EndpointClass::LiveStartup => (config.health_limit, "health"),
        EndpointClass::Auth => (config.auth_ip_limit, "auth"),
        EndpointClass::Upload => (config.upload_ip_limit, "ip"),
        EndpointClass::Search => (config.search_ip_limit, "ip"),
        EndpointClass::Stream => (config.stream_ip_limit, "ip"),
        EndpointClass::Default => (config.default_ip_limit, "ip"),
    }
}

fn user_limit(config: &RateLimitConfig, class: EndpointClass) -> Option<(u32, &'static str)> {
    match class {
        EndpointClass::Auth | EndpointClass::LiveStartup | EndpointClass::Ready => None,
        EndpointClass::Upload => Some((config.upload_user_limit, "user")),
        EndpointClass::Search => Some((config.search_user_limit, "user")),
        EndpointClass::Stream => Some((config.stream_user_limit, "user")),
        EndpointClass::Default => Some((config.default_user_limit, "user")),
    }
}

/// Only verified access tokens contribute an actor key (no unverified JWT bypass).
fn verified_actor(state: &AppState, authorization: Option<&str>) -> Option<(Uuid, Uuid)> {
    let provider = state.auth_provider()?;
    let token = authorization?
        .strip_prefix("Bearer ")
        .or_else(|| authorization?.strip_prefix("bearer "))?;
    let claims = provider.keys().verify_access_token(token).ok()?;
    let user_id = Uuid::parse_str(&claims.sub).ok()?;
    let org_id = Uuid::parse_str(&claims.org_id).ok()?;
    Some((org_id, user_id))
}

fn too_many_requests(request_id: &str, decision: &RateLimitDecision) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(
            ApiError::new("rate_limited", "Rate limit exceeded", request_id).with_details(json!({
                "limit": decision.limit,
                "remaining": decision.remaining,
                "resetSecs": decision.reset_secs,
                "scope": decision.scope,
                "quota": {
                    "limit": decision.limit,
                    "remaining": decision.remaining,
                    "resetSecs": decision.reset_secs,
                    "scope": decision.scope
                }
            })),
        ),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&decision.retry_after_secs.to_string()) {
        response.headers_mut().insert("retry-after", value);
    }
    attach_rate_headers(&mut response, decision);
    response
}

fn attach_rate_headers(response: &mut Response, decision: &RateLimitDecision) {
    if let Ok(value) = HeaderValue::from_str(&decision.limit.to_string()) {
        response.headers_mut().insert("x-ratelimit-limit", value);
    }
    if let Ok(value) = HeaderValue::from_str(&decision.remaining.to_string()) {
        response
            .headers_mut()
            .insert("x-ratelimit-remaining", value);
    }
    if let Ok(value) = HeaderValue::from_str(&decision.reset_secs.to_string()) {
        response.headers_mut().insert("x-ratelimit-reset", value);
    }
}

fn bad_proxy(request_id: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError::new(
            "invalid_forwarded_for",
            "Trusted proxy submitted an invalid X-Forwarded-For header",
            request_id,
        )),
    )
        .into_response()
}

fn cardinality_full(request_id: &str) -> Response {
    too_many_requests(
        request_id,
        &RateLimitDecision {
            allowed: false,
            limit: 0,
            remaining: 0,
            reset_secs: 1,
            retry_after_secs: 1,
            scope: "cardinality",
        },
    )
}

fn outcome_or_reject(
    outcome: CheckOutcome,
    request_id: &str,
) -> Result<RateLimitDecision, Box<Response>> {
    match outcome {
        CheckOutcome::Allowed(decision) => Ok(decision),
        CheckOutcome::Limited(decision) => Err(Box::new(too_many_requests(request_id, &decision))),
        CheckOutcome::CardinalityFull => Err(Box::new(cardinality_full(request_id))),
    }
}

pub async fn rate_limit_middleware(
    State(state): State<std::sync::Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let limiter = state.rate_limiter();
    if !limiter.config().enabled {
        return next.run(request).await;
    }

    let path = request.uri().path().to_string();
    let class = EndpointClass::classify(request.method(), &path);
    if class.is_cheap_exempt() && limiter.config().exempt_health {
        return next.run(request).await;
    }

    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    let now = Instant::now();
    let (ip_limit_n, ip_scope) = ip_limit(limiter.config(), class);
    let client_ip = match resolve_client_ip(peer, request.headers(), state.trusted_proxies()) {
        Ok(ip) => ip,
        Err(ClientIpError::SpoofedOrMissingForwarded) => {
            // Invalid forwarded metadata still consumes a budget keyed to the
            // trusted peer, otherwise malformed requests bypass every counter.
            let peer_ip = peer
                .map(|address| address.ip())
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
            match outcome_or_reject(
                limiter.check_keyed(
                    LimitKey::Ip { class, ip: peer_ip },
                    ip_limit_n,
                    ip_scope,
                    now,
                ),
                &request_id,
            ) {
                Ok(_) => return bad_proxy(&request_id),
                Err(response) => return *response,
            }
        }
    };

    let ip_decision = match outcome_or_reject(
        limiter.check_keyed(
            LimitKey::Ip {
                class,
                ip: client_ip,
            },
            ip_limit_n,
            ip_scope,
            now,
        ),
        &request_id,
    ) {
        Ok(decision) => decision,
        Err(response) => return *response,
    };

    // Auth endpoints remain IP-limited only. Unverified bearer never bypasses IP.
    let mut final_decision = ip_decision;
    if !class.is_auth() {
        let authorization = request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        if let Some((org_id, user_id)) = verified_actor(&state, authorization) {
            if let Some((user_limit_n, user_scope)) = user_limit(limiter.config(), class) {
                match outcome_or_reject(
                    limiter.check_keyed(
                        LimitKey::Actor {
                            class,
                            org_id,
                            user_id,
                        },
                        user_limit_n,
                        user_scope,
                        now,
                    ),
                    &request_id,
                ) {
                    Ok(user_decision) => {
                        // Surface the tighter remaining budget.
                        if user_decision.remaining < final_decision.remaining {
                            final_decision = user_decision;
                        }
                    }
                    Err(response) => return *response,
                }
            }
        }
    }

    let mut response = next.run(request).await;
    attach_rate_headers(&mut response, &final_decision);
    response
}

impl InMemoryRateLimiter {
    pub fn check_ip(&self, class: EndpointClass, ip: IpAddr, limit: u32) -> RateLimitDecision {
        match self.check_keyed(LimitKey::Ip { class, ip }, limit, "ip", Instant::now()) {
            CheckOutcome::Allowed(d) | CheckOutcome::Limited(d) => d,
            CheckOutcome::CardinalityFull => RateLimitDecision {
                allowed: false,
                limit,
                remaining: 0,
                reset_secs: 1,
                retry_after_secs: 1,
                scope: "cardinality",
            },
        }
    }

    pub fn check_actor(
        &self,
        class: EndpointClass,
        org_id: Uuid,
        user_id: Uuid,
        limit: u32,
    ) -> RateLimitDecision {
        match self.check_keyed(
            LimitKey::Actor {
                class,
                org_id,
                user_id,
            },
            limit,
            "user",
            Instant::now(),
        ) {
            CheckOutcome::Allowed(d) | CheckOutcome::Limited(d) => d,
            CheckOutcome::CardinalityFull => RateLimitDecision {
                allowed: false,
                limit,
                remaining: 0,
                reset_secs: 1,
                retry_after_secs: 1,
                scope: "cardinality",
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RateLimitConfig;

    #[test]
    fn fixed_window_blocks_and_reports_metadata() {
        let mut config = RateLimitConfig::production_defaults();
        config.window_secs = 60;
        config.max_keys = 16;
        let limiter = InMemoryRateLimiter::new(config);
        let ip = IpAddr::from([203, 0, 113, 9]);
        assert!(limiter.check_ip(EndpointClass::Default, ip, 2).allowed);
        assert!(limiter.check_ip(EndpointClass::Default, ip, 2).allowed);
        let blocked = limiter.check_ip(EndpointClass::Default, ip, 2);
        assert!(!blocked.allowed);
        assert_eq!(blocked.remaining, 0);
        assert!(blocked.retry_after_secs >= 1);
    }

    #[test]
    fn classifies_endpoint_families() {
        assert_eq!(
            EndpointClass::classify(&Method::GET, "/live"),
            EndpointClass::LiveStartup
        );
        assert_eq!(
            EndpointClass::classify(&Method::GET, "/ready"),
            EndpointClass::Ready
        );
        assert_eq!(
            EndpointClass::classify(&Method::POST, "/api/v1/auth/login"),
            EndpointClass::Auth
        );
    }

    #[test]
    fn max_keys_one_refuses_second_insert_without_evicting_first() {
        let mut config = RateLimitConfig::production_defaults();
        config.max_keys = 1;
        config.window_secs = 60;
        let limiter = InMemoryRateLimiter::new(config);
        let a = IpAddr::from([198, 51, 100, 1]);
        let b = IpAddr::from([198, 51, 100, 2]);
        assert!(limiter.check_ip(EndpointClass::Default, a, 100).allowed);
        let second = limiter.check_ip(EndpointClass::Default, b, 100);
        assert!(!second.allowed);
        assert_eq!(second.scope, "cardinality");
        assert_eq!(limiter.len(), 1);
        // Existing counter still advances.
        assert!(limiter.check_ip(EndpointClass::Default, a, 100).allowed);
        assert_eq!(limiter.len(), 1);
    }
}
