//! P1B-R06 findings: OpenAPI drift, CORS, dual budgets, readiness, envelopes.

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fileconv_server::api::openapi::{
    extra_operations, forbidden_markers, missing_operations, OPENAPI_YAML,
};
use fileconv_server::auth::jwt::JwtKeys;
use fileconv_server::auth::provider::PasswordAuthProvider;
use fileconv_server::config::{
    parse_trusted_proxies, Argon2Config, AuthConfig, CorsConfig, JwtAlgorithm, Profile,
    RateLimitConfig, RuntimeEndpoints, SecretString, ServerConfig, TrustedProxies,
};
use fileconv_server::db::pool::create_pool;
use fileconv_server::http::{router, AppState};
use fileconv_server::middleware::{EndpointClass, InMemoryRateLimiter};
use fileconv_server::routes::health::{FakeHealthProbes, ProbeReason};
use fileconv_server::state::RuntimeState;
use http_body_util::BodyExt;
use ipnet::IpNet;
use tower::ServiceExt;
use uuid::Uuid;

fn test_database_url() -> String {
    std::env::var("MARKHAND_TEST_DATABASE_URL").unwrap_or_else(|_| {
        "postgres://markhand_test:markhand_test_local@127.0.0.1:5432/markhand_test".into()
    })
}

fn test_auth_config() -> AuthConfig {
    AuthConfig {
        issuer: Some("https://issuer.markhand.test".into()),
        audience: Some("markhand-api".into()),
        signing_key: Some(SecretString::new("integration-test-signing-key-32b!")),
        alg: JwtAlgorithm::Hs256,
        kid: Some("test-kid-1".into()),
        access_token_ttl_secs: 900,
        refresh_token_ttl_secs: 3_600,
        argon2: Argon2Config {
            memory_kib: 8_192,
            time_cost: 1,
            parallelism: 1,
        },
    }
}

fn test_runtime() -> RuntimeState {
    RuntimeState::from_config(ServerConfig::test_with_endpoints(RuntimeEndpoints {
        database_url: SecretString::new(test_database_url()),
        qdrant_url: "http://127.0.0.1:1".into(),
        minio_url: "http://127.0.0.1:1".into(),
    }))
    .unwrap()
}

fn test_state() -> AppState {
    let pool = create_pool(&test_database_url()).expect("pool");
    AppState::from_parts(test_runtime(), pool, None).unwrap()
}

fn test_state_with_auth() -> (AppState, JwtKeys) {
    let pool = create_pool(&test_database_url()).expect("pool");
    let auth = test_auth_config();
    let keys = JwtKeys::from_auth(&auth).expect("jwt keys");
    let provider = PasswordAuthProvider::new(pool.clone(), auth.clone(), keys.clone());
    let state = AppState::from_parts(test_runtime(), pool, Some(provider)).unwrap();
    (state, keys)
}

async fn call(
    app: axum::Router,
    req: Request<Body>,
) -> (StatusCode, serde_json::Value, axum::http::HeaderMap) {
    let response = app.oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = if body.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| serde_json::json!({ "raw": String::from_utf8_lossy(&body) }))
    };
    (status, json, headers)
}

#[test]
fn openapi_structural_inventory_is_bidirectional_without_secrets() {
    assert!(
        missing_operations(OPENAPI_YAML).is_empty(),
        "missing: {:?}",
        missing_operations(OPENAPI_YAML)
    );
    assert!(
        extra_operations(OPENAPI_YAML).is_empty(),
        "extra: {:?}",
        extra_operations(OPENAPI_YAML)
    );
    assert!(forbidden_markers(OPENAPI_YAML).is_empty());
    assert!(OPENAPI_YAML.contains("text/event-stream"));
    assert!(OPENAPI_YAML.contains("bearerAuth"));
    assert!(OPENAPI_YAML.contains("/live"));
    assert!(OPENAPI_YAML.contains("head:"));
    assert!(OPENAPI_YAML.contains("X-RateLimit-Limit"));
    assert!(OPENAPI_YAML.contains("Retry-After"));
    assert!(OPENAPI_YAML.contains("application/octet-stream"));
    assert!(OPENAPI_YAML.contains("purpose"));
    assert!(OPENAPI_YAML.contains("citations"));
    assert!(
        OPENAPI_YAML.contains("'201'")
            || OPENAPI_YAML.contains("\"201\"")
            || OPENAPI_YAML.contains("createCollection")
    );
    // VersionMode wire field is `type`, not `kind`.
    assert!(OPENAPI_YAML.contains("VersionMode"));
    assert!(!OPENAPI_YAML.contains("kind:\n            type: string\n            enum:"));
}

#[tokio::test]
async fn request_id_validates_generates_and_echoes() {
    let app = router(test_state());
    let given = "550e8400-e29b-41d4-a716-446655440000";
    let (status, body, headers) = call(
        app.clone(),
        Request::builder()
            .uri("/live")
            .header("x-request-id", given)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-request-id").unwrap(), given);
    assert_eq!(body["requestId"], given);

    let (status, body, headers) = call(
        app,
        Request::builder()
            .uri("/live")
            .header("x-request-id", "not-a-uuid")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let generated = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .expect("generated request id");
    assert_ne!(generated, "not-a-uuid");
    assert!(Uuid::parse_str(generated).is_ok());
    assert_eq!(body["requestId"], generated);
}

#[test]
fn cors_rejects_invalid_method_token_and_wildcard() {
    let mut cors = CorsConfig::production_defaults();
    cors.allowed_origins = vec!["https://app.example".into()];
    assert!(cors.validate(Profile::Dev).is_ok());

    let mut wild = CorsConfig::production_defaults();
    wild.allowed_origins = vec!["*".into()];
    assert!(wild
        .validate(Profile::Prod)
        .unwrap_err()
        .contains("wildcard"));

    let mut bad_method = CorsConfig::production_defaults();
    bad_method.allowed_methods = vec!["GET".into(), "FOO".into()];
    assert!(bad_method.validate(Profile::Dev).is_err());
}

#[tokio::test]
async fn cors_true_preflight_only_for_known_route_and_origin() {
    // Default empty origin allow-list → deny unknown origin on true preflight.
    let app = router(test_state());

    // True preflight + unknown origin → 403 envelope.
    let (status, body, headers) = call(
        app.clone(),
        Request::builder()
            .method("OPTIONS")
            .uri("/api/v1/auth/me")
            .header("Origin", "https://evil.example")
            .header("Access-Control-Request-Method", "GET")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "cors_origin_denied");
    assert!(headers.get("access-control-allow-origin").is_none());

    // OPTIONS without ACRM is not a true preflight → pass/fallback (not 403 cors).
    let (status, body, _) = call(
        app.clone(),
        Request::builder()
            .method("OPTIONS")
            .uri("/api/v1/auth/me")
            .header("Origin", "https://evil.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_ne!(body["code"], "cors_origin_denied");
    assert!(
        status == StatusCode::METHOD_NOT_ALLOWED
            || status == StatusCode::NOT_FOUND
            || status == StatusCode::OK
            || status == StatusCode::NO_CONTENT
    );

    // True preflight for unknown route → pass/fallback, not synthetic 204.
    let (status, body, _) = call(
        app,
        Request::builder()
            .method("OPTIONS")
            .uri("/api/v1/does-not-exist")
            .header("Origin", "https://app.example")
            .header("Access-Control-Request-Method", "GET")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_ne!(status, StatusCode::NO_CONTENT);
    assert_ne!(body["code"], "cors_origin_denied");
}

#[tokio::test]
async fn cors_preflights_consume_the_auth_ip_budget() {
    let mut rate = RateLimitConfig::production_defaults();
    rate.enabled = true;
    rate.auth_ip_limit = 2;
    rate.window_secs = 60;
    let app = router(test_state().with_rate_limiter(InMemoryRateLimiter::new(rate)));

    for _ in 0..2 {
        let (status, body, headers) = call(
            app.clone(),
            Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/auth/me")
                .header("Origin", "https://evil.example")
                .header("Access-Control-Request-Method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "cors_origin_denied");
        assert!(headers.get("x-ratelimit-remaining").is_none());
    }

    let (status, body, headers) = call(
        app,
        Request::builder()
            .method("OPTIONS")
            .uri("/api/v1/auth/me")
            .header("Origin", "https://evil.example")
            .header("Access-Control-Request-Method", "GET")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "rate_limited");
    assert_eq!(body["details"]["scope"], "auth");
    assert!(headers.get("retry-after").is_some());
}

#[tokio::test]
async fn rate_limit_returns_429_with_limit_remaining_reset_and_retry_after() {
    let mut rate = RateLimitConfig::production_defaults();
    rate.enabled = true;
    rate.exempt_health = true;
    rate.default_ip_limit = 2;
    rate.window_secs = 60;
    let state = test_state().with_rate_limiter(InMemoryRateLimiter::new(rate));
    let app = router(state);

    for _ in 0..2 {
        let (status, _, _) = call(
            app.clone(),
            Request::builder()
                .uri("/api/v1/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, body, headers) = call(
        app,
        Request::builder()
            .uri("/api/v1/does-not-exist")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "rate_limited");
    assert!(headers.get("retry-after").is_some());
    assert!(headers.get("x-ratelimit-limit").is_some());
    assert!(headers.get("x-ratelimit-remaining").is_some());
    assert!(headers.get("x-ratelimit-reset").is_some());
    assert_eq!(body["details"]["remaining"], 0);
    assert!(body["details"]["limit"].as_u64().unwrap() >= 1);
    assert!(body["details"]["resetSecs"].as_u64().unwrap() >= 1);
}

#[test]
fn rate_limiter_max_keys_one_never_evicts_existing_counter() {
    let mut rate = RateLimitConfig::production_defaults();
    rate.max_keys = 1;
    rate.window_secs = 60;
    let limiter = InMemoryRateLimiter::new(rate);
    let a = IpAddr::from([198, 51, 100, 1]);
    let b = IpAddr::from([198, 51, 100, 2]);
    assert!(limiter.check_ip(EndpointClass::Default, a, 100).allowed);
    let second = limiter.check_ip(EndpointClass::Default, b, 100);
    assert!(!second.allowed);
    assert_eq!(second.scope, "cardinality");
    assert_eq!(limiter.len(), 1);
    assert!(limiter.check_ip(EndpointClass::Default, a, 100).allowed);
    assert_eq!(limiter.len(), 1);
}

#[tokio::test]
async fn dual_budgets_always_enforce_ip_and_verified_actor_user_budget() {
    let (base, keys) = test_state_with_auth();
    let mut rate = RateLimitConfig::production_defaults();
    rate.enabled = true;
    rate.exempt_health = true;
    rate.default_ip_limit = 100;
    rate.default_user_limit = 2;
    let state = base.with_rate_limiter(InMemoryRateLimiter::new(rate));
    let app = router(state);
    let org = Uuid::new_v4();
    let user = Uuid::new_v4();
    let token = keys
        .sign_access_token(user, org, Uuid::new_v4())
        .unwrap()
        .expose()
        .to_string();

    for _ in 0..2 {
        let (status, _, _) = call(
            app.clone(),
            Request::builder()
                .uri("/api/v1/does-not-exist")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, body, _) = call(
        app.clone(),
        Request::builder()
            .uri("/api/v1/does-not-exist")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["details"]["scope"], "user");

    // Unverified JWT must not bypass IP budgeting (auth-class stays IP-only separately).
    let mut auth_rate = RateLimitConfig::production_defaults();
    auth_rate.enabled = true;
    auth_rate.auth_ip_limit = 2;
    let (auth_state, _) = test_state_with_auth();
    let app = router(auth_state.with_rate_limiter(InMemoryRateLimiter::new(auth_rate)));
    for _ in 0..2 {
        let (status, _, _) = call(
            app.clone(),
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("content-type", "application/json")
                .header("Authorization", "Bearer not-a-real-jwt")
                .body(Body::from(r#"{"email":"a@b.c","password":"x"}"#))
                .unwrap(),
        )
        .await;
        assert_ne!(status, StatusCode::TOO_MANY_REQUESTS);
    }
    let (status, body, _) = call(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("content-type", "application/json")
            .header("Authorization", "Bearer not-a-real-jwt")
            .body(Body::from(r#"{"email":"a@b.c","password":"x"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["details"]["scope"], "auth");
}

#[tokio::test]
async fn trusted_proxy_exact_spoof_and_right_to_left_xff() {
    let proxies = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
    let mut rate = RateLimitConfig::production_defaults();
    rate.enabled = true;
    rate.exempt_health = false;
    rate.health_limit = 100;
    let state = test_state()
        .with_trusted_proxies(proxies)
        .with_rate_limiter(InMemoryRateLimiter::new(rate));
    let app = router(state);

    // Spoof: untrusted peer XFF ignored — request succeeds as peer IP.
    let mut req = Request::builder()
        .uri("/live")
        .header("x-forwarded-for", "198.51.100.20")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [203, 0, 113, 10],
            443,
        ))));
    let (status, _, _) = call(app.clone(), req).await;
    assert_eq!(status, StatusCode::OK);

    // Trusted peer missing XFF → reject.
    let mut req = Request::builder().uri("/live").body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [10, 0, 0, 2],
            443,
        ))));
    let (status, body, _) = call(app.clone(), req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_forwarded_for");

    // Trusted peer + single overwritten client IP.
    let mut req = Request::builder()
        .uri("/live")
        .header("x-forwarded-for", "198.51.100.20")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [10, 0, 0, 2],
            443,
        ))));
    let (status, _, _) = call(app.clone(), req).await;
    assert_eq!(status, StatusCode::OK);

    // Right-to-left walk with trusted hop then client.
    let mut req = Request::builder()
        .uri("/live")
        .header("x-forwarded-for", "198.51.100.7, 203.0.113.9, 10.0.0.9")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [10, 0, 0, 2],
            443,
        ))));
    let (status, _, _) = call(app.clone(), req).await;
    assert_eq!(status, StatusCode::OK);

    // Multiple XFF header fields are rejected (ambiguous wire order).
    let mut req = Request::builder()
        .uri("/live")
        .header("x-forwarded-for", "198.51.100.7")
        .body(Body::empty())
        .unwrap();
    req.headers_mut()
        .append("x-forwarded-for", "203.0.113.9".parse().unwrap());
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [10, 0, 0, 2],
            443,
        ))));
    let (status, body, _) = call(app, req).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_forwarded_for");

    assert!(parse_trusted_proxies("0.0.0.0/0")
        .unwrap_err()
        .contains("/0"));
}

#[tokio::test]
async fn invalid_forwarded_for_from_a_trusted_peer_is_rate_limited() {
    let proxies = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
    let mut rate = RateLimitConfig::production_defaults();
    rate.enabled = true;
    rate.exempt_health = false;
    rate.health_limit = 2;
    rate.window_secs = 60;
    let app = router(
        test_state()
            .with_trusted_proxies(proxies)
            .with_rate_limiter(InMemoryRateLimiter::new(rate)),
    );

    let request = || {
        let mut request = Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(SocketAddr::from((
                [10, 0, 0, 2],
                443,
            ))));
        request
    };

    for _ in 0..2 {
        let (status, body, _) = call(app.clone(), request()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_forwarded_for");
    }
    let (status, body, headers) = call(app, request()).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "rate_limited");
    assert_eq!(body["details"]["scope"], "health");
    assert!(headers.get("retry-after").is_some());
}

#[test]
fn prod_rejects_disabled_limiter() {
    let mut cfg = RateLimitConfig::production_defaults();
    cfg.enabled = false;
    assert!(cfg
        .validate_for_profile(Profile::Prod)
        .unwrap_err()
        .contains("RATE_LIMIT_ENABLED"));
    assert!(cfg.validate_for_profile(Profile::Dev).is_ok());
}

#[tokio::test]
async fn readiness_signature_mismatch_and_reconcile_blocked() {
    let state = test_state()
        .with_fake_probes(FakeHealthProbes::all_ok())
        .with_embedding_init_error(ProbeReason::Signature);
    let app = router(state);
    let (status, body, _) = call(
        app,
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "dependency_unavailable");

    let mut probes = FakeHealthProbes::all_ok();
    probes.reconciliation = false;
    let blocked = test_state().with_fake_probes(probes);
    assert!(!blocked.reconciliation_gate().is_ready());
    let (status, _, _) = call(
        router(blocked),
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let recovered = test_state().with_fake_probes(FakeHealthProbes::all_ok());
    recovered.reconciliation_gate().set_ready(true);
    let (status, body, _) = call(
        router(recovered),
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn readiness_reports_embedding_initialization_failure() {
    let state = test_state()
        .with_fake_probes(FakeHealthProbes::all_ok())
        .with_embedding_init_error(ProbeReason::Embedding);
    let (status, body, _) = call(
        router(state),
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "dependency_unavailable");
    assert!(!body.to_string().to_ascii_lowercase().contains("embedding"));
}

#[tokio::test]
async fn reconciliation_fence_changes_are_not_hidden_by_readiness_cache() {
    let probes = FakeHealthProbes::all_ok();
    let runs = probes.runs.clone();
    let state = test_state().with_fake_probes(probes);
    let gate = std::sync::Arc::clone(state.reconciliation_gate());
    let app = router(state);

    let (first, _, _) = call(
        app.clone(),
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(first, StatusCode::OK);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);

    gate.set_ready(false);
    let (second, body, _) = call(
        app,
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(second, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "dependency_unavailable");
    assert_eq!(
        runs.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "dynamic fence should fail before cached dependency probes"
    );
}

#[tokio::test]
async fn ready_is_rate_limited_and_cached_while_live_exempt() {
    let mut rate = RateLimitConfig::production_defaults();
    rate.enabled = true;
    rate.exempt_health = true;
    rate.health_limit = 3;
    rate.window_secs = 60;
    let state = test_state()
        .with_fake_probes(FakeHealthProbes::all_ok())
        .with_rate_limiter(InMemoryRateLimiter::new(rate));
    let app = router(state);

    // Live remains exempt under flood.
    for _ in 0..8 {
        let (status, _, _) = call(
            app.clone(),
            Request::builder().uri("/live").body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Ready is not exempt — after budget, 429.
    let mut last = StatusCode::OK;
    for _ in 0..6 {
        let (status, _, _) = call(
            app.clone(),
            Request::builder()
                .uri("/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        last = status;
        if status == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);

    // Caching: second ready within TTL must not re-run dependency probes.
    let probes = FakeHealthProbes::all_ok();
    let runs = probes.runs.clone();
    let state = test_state()
        .with_fake_probes(probes)
        .with_rate_limiter(InMemoryRateLimiter::new({
            let mut rate = RateLimitConfig::production_defaults();
            rate.health_limit = 1_000;
            rate
        }));
    let app = router(state);
    let (first, _, _) = call(
        app.clone(),
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(first, StatusCode::OK);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    let (second, _, _) = call(
        app,
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(second, StatusCode::OK);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn malformed_json_returns_canonical_envelope() {
    let (state, _) = test_state_with_auth();
    let app = router(state);
    let (status, body, _) = call(
        app,
        Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("content-type", "application/json")
            .body(Body::from("{not-json"))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "validation_failed");
    assert!(body["requestId"].as_str().is_some());
    assert!(body["message"].as_str().is_some());
}

#[tokio::test]
async fn readiness_fails_and_recovers_with_fake_probes_liveness_unaffected() {
    let mut probes = FakeHealthProbes::all_ok();
    probes.postgres = false;
    let state = test_state().with_fake_probes(probes);
    let app = router(state);

    let (live_status, _, _) = call(
        app.clone(),
        Request::builder().uri("/live").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(live_status, StatusCode::OK);

    let (ready_status, body, _) = call(
        app,
        Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(ready_status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "dependency_unavailable");
    let body_text = body.to_string();
    assert!(!body_text.to_ascii_lowercase().contains("postgres"));
}

#[tokio::test]
async fn startup_completed_and_head_live_supported() {
    let app = router(test_state());
    let (status, body, _) = call(
        app.clone(),
        Request::builder()
            .uri("/startup")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["completed"], true);

    let response = app
        .oneshot(
            Request::builder()
                .method("HEAD")
                .uri("/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn startup_exposes_starting_and_degraded_contract_states() {
    let starting = AppState::new(test_runtime()).expect("starting app state");
    let (status, body, _) = call(
        router(starting),
        Request::builder()
            .uri("/startup")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "starting");
    assert_eq!(body["completed"], false);
    assert_eq!(body["degraded"], false);

    let degraded = AppState::new(test_runtime()).expect("degraded app state");
    degraded.startup_state().mark_completed(true);
    let (status, body, _) = call(
        router(degraded),
        Request::builder()
            .uri("/startup")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["completed"], true);
    assert_eq!(body["degraded"], true);
}
