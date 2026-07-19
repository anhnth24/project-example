use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{MatchedPath, State};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use tracing::{info_span, Instrument};

use crate::http::AppState;
use crate::middleware::request_id::RequestId;

const UNMATCHED_ROUTE: &str = "unmatched";

pub async fn record_http_metrics(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().as_str().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_string())
        .unwrap_or_else(|| UNMATCHED_ROUTE.into());
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.0.clone())
        .unwrap_or_default();
    let span = info_span!(
        "http.request",
        request_id = %request_id,
        http.method = %method,
        http.route = %route
    );
    async move {
        let started = Instant::now();
        let response = next.run(request).await;
        let status = response.status().as_u16();
        let elapsed = started.elapsed().as_secs_f64();
        state
            .metrics()
            .record_http_request(&route, &method, status, elapsed);
        tracing::info!(
            request_id = %request_id,
            http.method = %method,
            http.route = %route,
            http.status = status,
            duration_ms = (elapsed * 1000.0).round() as u64,
            "http request completed"
        );
        response
    }
    .instrument(span)
    .await
}
