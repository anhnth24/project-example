//! Packaged-server evidence for P2-16 (build + serve the SPA).
//!
//! Exercises the *real* `http::router(AppState)` — the full middleware stack
//! (request-id, write-gate, CORS, rate-limit) wraps the SPA routes exactly
//! like every `/api/v1` route, not a bare `spa::spa_router` in isolation
//! (that unit-level guarantee is covered by `src/spa.rs`'s own tests).
//!
//! Hermetic by construction, no live Postgres/MinIO required:
//! - `/api/v1/health/*` is write-gate-exempt (see
//!   `middleware::write_gate::is_write_gate_exempt`), so an *unmatched*
//!   sub-path under it (`/api/v1/health/does-not-exist`) reaches the router's
//!   fallback without ever touching the DB pool, giving a deterministic 404
//!   without a live database.
//! - Every other assertion here (deep-link fallback, asset cache headers) is
//!   entirely outside `/api/v1`, which `is_write_gate_exempt` and
//!   `baseline_ip_rate_limit` both already treat as ops-adjacent/exempt.
//!
//! Mutation-test transcript for these guarantees lives in the P2-16 report,
//! not in this file — each assertion below was confirmed to go red by
//! independently breaking the corresponding guard in `src/spa.rs` (dropping
//! the `/api/` prefix check, swapping the two Cache-Control constants,
//! deleting one security-header `insert`/layer at a time) and rerunning.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fileconv_server::spa::WEB_DIST_ENV;
use http_body_util::BodyExt;
use tower::ServiceExt;

/// `MARKHAND_WEB_DIST_DIR` is a process-global env var; serialize every test
/// in this binary that touches it so parallel `#[tokio::test]` execution
/// can't interleave a set/remove from one test into another's window.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Never-reachable-but-syntactically-valid DB URL — matches the pattern
/// already used by `http.rs`'s own `liveness_has_a_contract_compliant_body`
/// unit test. `deadpool_postgres::Pool` connects lazily, so building the
/// pool/state/router never dials out; only a handler that actually needs a
/// connection would. Every path this test hits is write-gate-exempt.
fn hermetic_pool() -> deadpool_postgres::Pool {
    fileconv_server::db::pool::create_pool(
        "postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test",
    )
    .expect("pool construction is lazy and infallible for a well-formed URL")
}

fn write_fixture_dist(dir: &std::path::Path) -> String {
    std::fs::create_dir_all(dir.join("assets")).expect("mkdir assets");
    std::fs::write(
        dir.join("index.html"),
        b"<!doctype html><html><body><div id=\"root\">markhand-spa-shell</div></body></html>",
    )
    .expect("write index.html");
    let asset_name = "index-fixturehash123.js";
    std::fs::write(
        dir.join("assets").join(asset_name),
        b"console.log('markhand');",
    )
    .expect("write asset");
    asset_name.to_string()
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn server_boots_and_serves_api_without_web_dist_present() {
    // The lock is scoped to env-var manipulation plus the (synchronous)
    // router build, which is the only window where `WEB_DIST_ENV` is read —
    // `resolve_web_dist_dir` runs inside `build_router`, never later. Holding
    // it across the awaits below would serialize nothing useful and trips
    // clippy's `await_holding_lock`, which CI runs as `-D warnings`.
    let app = {
        let _guard = env_lock();
        std::env::remove_var(WEB_DIST_ENV);
        // Default lookup ("web/dist" relative to CWD) must not resolve here —
        // `cargo test`'s CWD for this binary is `crates/server`, which has none.
        assert!(
            fileconv_server::spa::resolve_web_dist_dir().is_none(),
            "test assumes no web/dist is reachable from the crate directory"
        );
        common::build_router(
            hermetic_pool(),
            "postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test",
            None,
        )
    };

    // The API keeps working standalone: this is the "serving is optional"
    // guarantee (packaged server / dev / CI without a prior `pnpm build`).
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // And a UI path gets axum's bare 404 (no SPA shell mounted at all),
    // rather than any kind of html response.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/library/anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn packaged_server_deep_link_api_404_and_asset_cache_policy() {
    let dist = tempfile::tempdir().expect("tempdir");
    let asset_name = write_fixture_dist(dist.path());

    // Scoped for the same reason as the test above: set-then-build is the
    // whole critical section, and the guard must not cross an await.
    let app = {
        let _guard = env_lock();
        std::env::set_var(WEB_DIST_ENV, dist.path());
        common::build_router(
            hermetic_pool(),
            "postgres://markhand_app:markhand_app@127.0.0.1:5432/markhand_test",
            None,
        )
    };

    // --- Deep-link refresh: an unregistered UI route serves the SPA shell.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/library/9f2c1e10-aaaa-4bbb-8ccc-123456789abc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "deep link must serve the SPA shell"
    );
    let headers = response.headers().clone();
    assert_eq!(
        headers.get(axum::http::header::CONTENT_TYPE).unwrap(),
        "text/html; charset=utf-8"
    );
    assert_eq!(
        headers.get(axum::http::header::CACHE_CONTROL).unwrap(),
        "no-cache, must-revalidate",
        "HTML shell must be revalidated every load, never cached"
    );
    assert_eq!(
        headers
            .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
            .unwrap(),
        "nosniff"
    );
    assert_eq!(
        headers.get(axum::http::header::X_FRAME_OPTIONS).unwrap(),
        "DENY"
    );
    assert!(headers.contains_key(axum::http::header::CONTENT_SECURITY_POLICY));
    assert!(headers.contains_key(axum::http::header::REFERRER_POLICY));
    let body = body_string(response).await;
    assert!(body.contains("markhand-spa-shell"));

    // --- API 404 behaviour: an unmatched `/api/v1` path is never swallowed
    // by the SPA fallback. Uses a write-gate-exempt sub-path
    // (`/api/v1/health/...`) so this stays hermetic (no live Postgres) while
    // still going through the *full* router (request-id → write-gate → CORS
    // → rate-limit → fallback), not a bare `spa_router`.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "unmatched /api/v1 path must 404, never fall through to the SPA shell"
    );
    let body = body_string(response).await;
    assert!(
        !body.contains("markhand-spa-shell"),
        "unmatched API path leaked the SPA shell body: {body}"
    );

    // --- Asset cache policy: hashed assets are long-cached and immutable.
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/assets/{asset_name}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .unwrap(),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
            .unwrap(),
        "nosniff"
    );
    assert!(response
        .headers()
        .contains_key(axum::http::header::CONTENT_SECURITY_POLICY));

    std::env::remove_var(WEB_DIST_ENV);
}
