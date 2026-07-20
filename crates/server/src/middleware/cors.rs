use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ORIGIN, VARY,
};
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::http::AppState;

const ALLOWED_METHODS: &str = "GET,POST,PUT,PATCH,DELETE,OPTIONS";
const ALLOWED_HEADERS: &str = "Authorization,Content-Type,Idempotency-Key,Last-Event-ID";

pub async fn cors(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let allowed_origin = request
        .headers()
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .filter(|origin| {
            state
                .runtime()
                .config()
                .cors_allowed_origins()
                .iter()
                .any(|allowed| allowed == origin)
        })
        .map(str::to_string);

    if request.method() == Method::OPTIONS {
        let mut response = StatusCode::NO_CONTENT.into_response();
        if let Some(origin) = allowed_origin.as_deref() {
            apply_cors_headers(response.headers_mut(), origin);
        }
        return response;
    }

    let mut response = next.run(request).await;
    if let Some(origin) = allowed_origin.as_deref() {
        apply_cors_headers(response.headers_mut(), origin);
    }
    response
}

fn apply_cors_headers(headers: &mut axum::http::HeaderMap, origin: &str) {
    if let Ok(origin) = HeaderValue::from_str(origin) {
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(ALLOWED_METHODS),
        );
        headers.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(ALLOWED_HEADERS),
        );
        headers.append(VARY, HeaderValue::from_static("Origin"));
    }
}
