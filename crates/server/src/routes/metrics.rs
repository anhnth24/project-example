use std::sync::Arc;

use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::http::AppState;

const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4";

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/metrics", get(metrics))
}

async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    if !state.runtime().config().metrics_enabled() {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Production deployments must network-restrict this unauthenticated scrape endpoint.
    let mut response = state.metrics().render_prometheus().into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(PROMETHEUS_CONTENT_TYPE),
    );
    response
}
