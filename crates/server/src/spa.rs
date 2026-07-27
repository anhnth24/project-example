//! Static SPA serving for the built `web/dist` bundle (P2-16).
//!
//! Serving is **optional**: [`resolve_web_dist_dir`] / [`spa_router`] return
//! `None` when the directory (or its `index.html`) is missing, and the
//! caller (`http::router`) simply skips mounting any static routes in that
//! case. Tests, `cargo test`, CI and local `cargo run` without a prior
//! `pnpm --dir web build` all keep working as API-only — measured by
//! `tests/spa_static_serving.rs::server_starts_and_serves_api_without_web_dist`.
//!
//! # History-fallback boundary (the load-bearing guarantee of this module)
//!
//! [`spa_fallback`] is installed as the router's *global* fallback, so it
//! runs for every request that didn't match an explicit route — that
//! includes typo'd/removed `/api/v1/*` paths, not just real UI deep links.
//! It therefore starts by checking the request path and refuses to answer
//! for anything under `/api/` or `/metrics`: those fall through to a plain
//! 404, exactly like an unmatched route would without this module mounted
//! at all. Only paths outside that reserved space get the SPA shell.
//!
//! This is asserted by `tests/spa_static_serving.rs`, and the report for
//! P2-16 records a mutation run that makes the fallback greedy (drops the
//! `/api/` guard) to confirm the API-404 test goes red without it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use tower::ServiceBuilder;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

/// Overrides the SPA directory; unset/empty falls back to `web/dist`
/// relative to the process CWD (repo root for `cargo run`, compose/Docker
/// WORKDIR in deployment — see `deploy/Dockerfile.server`).
pub const WEB_DIST_ENV: &str = "MARKHAND_WEB_DIST_DIR";

/// Strict CSP for the SPA shell and its static assets.
///
/// Narrowest policy that still loads the app as shipped today:
/// - `script-src 'self'` — the Vite build emits only `<script type="module"
///   src="/assets/...">`, no inline scripts, so no `unsafe-inline` is needed.
/// - `style-src`/`font-src` allow `fonts.googleapis.com`/`fonts.gstatic.com`
///   because `web/src/styles.css` currently loads Caprasimo/Figtree via a
///   CSS `@import` from Google Fonts (checked 2026-07-27). This is the one
///   deliberate cross-origin allowance in an otherwise `'self'`-only policy.
///   **Product decision handed back, not made silently**: the alternative is
///   self-hosting those two font families under `web/dist` and dropping this
///   allowance entirely — narrower, but requires a `web/src` change outside
///   this crate's ownership. Until that happens, this is the CSP.
/// - no `data:`/`unsafe-inline` anywhere; `object-src`/`base-uri`/
///   `frame-ancestors` are all `'none'`.
const CONTENT_SECURITY_POLICY: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self'; ",
    "style-src 'self' https://fonts.googleapis.com; ",
    "font-src 'self' https://fonts.gstatic.com; ",
    "img-src 'self'; ",
    "connect-src 'self'; ",
    "object-src 'none'; ",
    "base-uri 'none'; ",
    "frame-ancestors 'none'; ",
    "form-action 'self'",
);

/// `X-Frame-Options` is kept alongside CSP `frame-ancestors` for older UAs
/// that respect the former but not the latter.
const X_FRAME_OPTIONS: &str = "DENY";
const X_CONTENT_TYPE_OPTIONS: &str = "nosniff";
/// Same-origin destinations get the full URL; cross-origin ones get only the
/// origin — avoids leaking document paths/query strings off-site.
const REFERRER_POLICY: &str = "strict-origin-when-cross-origin";

/// HTML must be revalidated every load (SPA shell references hashed asset
/// URLs baked in at build time; caching the shell itself would pin clients
/// to a stale asset manifest after a deploy).
const HTML_CACHE_CONTROL: &str = "no-cache, must-revalidate";
/// Hashed asset filenames (`index-<hash>.js`) make the content at a given
/// URL permanently immutable — safe to cache for a year, per plan P2.9/P2.7.
const ASSET_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// Resolves the directory to serve, or `None` if serving should stay off.
///
/// `MARKHAND_WEB_DIST_DIR` wins when set and non-empty (used by deploy —
/// see `deploy/Dockerfile.server`/`deploy/compose.poc.yml`); otherwise falls
/// back to `web/dist` relative to CWD *only if it exists*. The latter never
/// fires for `cargo test` (Cargo runs test binaries with CWD = the crate
/// directory, `crates/server`, which has no `web/dist`), so test/CI runs
/// stay API-only by construction rather than by env-var discipline.
pub fn resolve_web_dist_dir() -> Option<PathBuf> {
    if let Ok(configured) = std::env::var(WEB_DIST_ENV) {
        let trimmed = configured.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    let default = PathBuf::from("web/dist");
    if default.is_dir() {
        Some(default)
    } else {
        None
    }
}

/// Built SPA index document, read once at startup.
struct SpaIndex {
    html: Vec<u8>,
}

/// Builds the SPA sub-router (`/assets/*` + history-fallback), or `None`
/// when `dist_dir/index.html` is absent/unreadable — the caller must then
/// skip mounting this at all so the API keeps serving standalone.
pub fn spa_router<S>(dist_dir: &Path) -> Option<Router<S>>
where
    S: Clone + Send + Sync + 'static,
{
    let index_path = dist_dir.join("index.html");
    let html = match std::fs::read(&index_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(
                target: "spa",
                path = %index_path.display(),
                error = %error,
                "web dist directory configured but index.html is unreadable; serving API only"
            );
            return None;
        }
    };
    let index = Arc::new(SpaIndex { html });

    let assets_dir = dist_dir.join("assets");
    let assets_service = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static(ASSET_CACHE_CONTROL),
        ))
        .layer(security_header_layer(
            header::X_CONTENT_TYPE_OPTIONS,
            X_CONTENT_TYPE_OPTIONS,
        ))
        .layer(security_header_layer(
            header::X_FRAME_OPTIONS,
            X_FRAME_OPTIONS,
        ))
        .layer(security_header_layer(
            header::REFERRER_POLICY,
            REFERRER_POLICY,
        ))
        .layer(security_header_layer(
            header::CONTENT_SECURITY_POLICY,
            CONTENT_SECURITY_POLICY,
        ))
        .service(ServeDir::new(assets_dir));

    let fallback_index = index.clone();

    Some(
        Router::new()
            .nest_service("/assets", assets_service)
            .fallback(move |method: Method, uri: Uri| {
                let index = fallback_index.clone();
                async move { spa_fallback(&index, &method, &uri) }
            }),
    )
}

fn security_header_layer(
    name: HeaderName,
    value: &'static str,
) -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(name, HeaderValue::from_static(value))
}

/// Global router fallback. Reached for any request that matched no explicit
/// route (auth/collections/.../openapi.yaml, `/metrics`, `/assets/*`), which
/// includes non-existent `/api/v1/*` paths. Guarding on the path prefix
/// first is what keeps the API's 404 surface intact — see module docs.
fn spa_fallback(index: &SpaIndex, method: &Method, uri: &Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") || path == "/metrics" {
        // Never serve the SPA shell for API/ops surfaces — preserve the
        // plain "no route matched" 404 those paths would get without this
        // module mounted at all.
        return StatusCode::NOT_FOUND.into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(index.html.clone())
    };
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(HTML_CACHE_CONTROL),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static(X_CONTENT_TYPE_OPTIONS),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static(X_FRAME_OPTIONS),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static(REFERRER_POLICY),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected header values, spelled out here independently of the
    // production constants above. Asserting a response header against the
    // very `const` that produced it is a tautology: change the constant and
    // both sides move together, so a swapped cache policy or a weakened CSP
    // stays green. Mutation-checked: flipping HTML_CACHE_CONTROL to the
    // immutable value left every test passing before these literals existed.
    // These are what actually gate P2.9's "HTML revalidates, hashed assets
    // immutable" and P2.7's header set — if you change a production constant
    // on purpose, this block must change too, deliberately.
    const EXPECT_HTML_CACHE: &str = "no-cache, must-revalidate";
    const EXPECT_ASSET_CACHE: &str = "public, max-age=31536000, immutable";
    const EXPECT_NOSNIFF: &str = "nosniff";
    const EXPECT_FRAME: &str = "DENY";
    const EXPECT_REFERRER: &str = "strict-origin-when-cross-origin";
    const EXPECT_CSP: &str = "default-src 'self'; script-src 'self'; \
style-src 'self' https://fonts.googleapis.com; \
font-src 'self' https://fonts.gstatic.com; img-src 'self'; \
connect-src 'self'; object-src 'none'; base-uri 'none'; \
frame-ancestors 'none'; form-action 'self'";

    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    fn write_dist(dir: &Path) {
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("index.html"),
            b"<!doctype html><html><body>spa shell</body></html>",
        )
        .unwrap();
        std::fs::write(
            dir.join("assets").join("index-deadbeef.js"),
            b"console.log(1)",
        )
        .unwrap();
    }

    // `resolve_web_dist_dir` reads a process-global env var; serialize the
    // two tests that touch it so they can't interleave (cargo test runs
    // `#[test]` fns concurrently within one process by default).
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn resolve_returns_none_without_env_or_default_dir() {
        let _guard = env_lock();
        // Isolate from whatever the process CWD happens to be (this test
        // binary's CWD is the crate dir, which has no web/dist — but assert
        // it explicitly rather than relying on that as an accident).
        std::env::remove_var(WEB_DIST_ENV);
        let cwd_default = PathBuf::from("web/dist");
        assert!(
            !cwd_default.is_dir(),
            "test assumes no web/dist under the crate directory"
        );
        assert!(resolve_web_dist_dir().is_none());
    }

    #[test]
    fn resolve_honors_env_override_even_when_missing() {
        let _guard = env_lock();
        std::env::set_var(WEB_DIST_ENV, "/nonexistent/does/not/exist");
        // The env var always wins when set/non-empty; spa_router (not
        // resolve_web_dist_dir) is what decides "usable" by checking
        // index.html, so a bad override degrades there, not here.
        assert_eq!(
            resolve_web_dist_dir(),
            Some(PathBuf::from("/nonexistent/does/not/exist"))
        );
        std::env::remove_var(WEB_DIST_ENV);
    }

    #[test]
    fn spa_router_is_none_when_index_html_missing() {
        let dir = tempfile::tempdir().unwrap();
        // Directory exists but is empty — no index.html.
        assert!(spa_router::<()>(dir.path()).is_none());
    }

    #[tokio::test]
    async fn fallback_serves_index_html_for_ui_deep_link() {
        let dir = tempfile::tempdir().unwrap();
        write_dist(dir.path());
        let app = spa_router::<()>(dir.path()).expect("router");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/library/some-collection-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            EXPECT_HTML_CACHE
        );
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            EXPECT_NOSNIFF
        );
        assert_eq!(
            response.headers().get(header::X_FRAME_OPTIONS).unwrap(),
            EXPECT_FRAME
        );
        assert_eq!(
            response.headers().get(header::REFERRER_POLICY).unwrap(),
            EXPECT_REFERRER
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .unwrap(),
            EXPECT_CSP
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("spa shell"));
    }

    #[tokio::test]
    async fn fallback_never_swallows_unmatched_api_path() {
        let dir = tempfile::tempdir().unwrap();
        write_dist(dir.path());
        let app = spa_router::<()>(dir.path()).expect("router");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            body.is_empty(),
            "unmatched /api/v1 path must not receive the SPA shell body"
        );
    }

    #[tokio::test]
    async fn fallback_never_swallows_metrics() {
        let dir = tempfile::tempdir().unwrap();
        write_dist(dir.path());
        let app = spa_router::<()>(dir.path()).expect("router");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn asset_is_served_immutable_with_security_headers() {
        let dir = tempfile::tempdir().unwrap();
        write_dist(dir.path());
        let app = spa_router::<()>(dir.path()).expect("router");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/assets/index-deadbeef.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            EXPECT_ASSET_CACHE
        );
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            EXPECT_NOSNIFF
        );
        assert_eq!(
            response.headers().get(header::X_FRAME_OPTIONS).unwrap(),
            EXPECT_FRAME
        );
        assert_eq!(
            response.headers().get(header::REFERRER_POLICY).unwrap(),
            EXPECT_REFERRER
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .unwrap(),
            EXPECT_CSP
        );
    }

    #[tokio::test]
    async fn missing_asset_is_a_plain_404_not_the_spa_shell() {
        let dir = tempfile::tempdir().unwrap();
        write_dist(dir.path());
        let app = spa_router::<()>(dir.path()).expect("router");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/assets/does-not-exist.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("spa shell"));
    }

    #[tokio::test]
    async fn non_get_head_ui_path_is_method_not_allowed() {
        let dir = tempfile::tempdir().unwrap();
        write_dist(dir.path());
        let app = spa_router::<()>(dir.path()).expect("router");
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/library/x")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
