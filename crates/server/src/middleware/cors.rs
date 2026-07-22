//! Conservative exact-origin CORS middleware.
//!
//! Only true browser preflights (`OPTIONS` + `Origin` + `Access-Control-Request-Method`)
//! for a known wired route/method are answered here. All other requests pass through;
//! successful responses still receive CORS headers when the Origin is allow-listed.

use axum::extract::Request;
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::openapi::is_wired_operation;
use crate::api::ApiError;
use crate::config::CorsConfig;
use crate::middleware::request_id::RequestId;

const VARY_CORS: &[&str] = &[
    "Origin",
    "Access-Control-Request-Method",
    "Access-Control-Request-Headers",
];

/// Layer function applying exact-origin CORS policy from config.
pub async fn cors_layer(config: CorsConfig, request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let acrm = request
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_METHOD)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_ascii_uppercase());
    let is_true_preflight =
        request.method() == Method::OPTIONS && origin.is_some() && acrm.is_some();
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    if is_true_preflight {
        let path = request.uri().path();
        let requested = acrm.as_deref().unwrap_or_default();
        let method_allowed = config
            .allowed_methods
            .iter()
            .any(|method| method.eq_ignore_ascii_case(requested));
        // Only intercept preflights for known wired route+method; otherwise fall through.
        if !method_allowed || !is_wired_operation(requested, path) {
            let mut response = next.run(request).await;
            apply_cors_headers(&config, origin.as_deref(), response.headers_mut());
            return response;
        }
        // Run the inner edge middleware before synthesizing the preflight response.
        // The routed OPTIONS fallback has no business side effects, while this keeps
        // preflights inside trusted-proxy validation and IP rate budgets.
        let downstream = next.run(request).await;
        if matches!(
            downstream.status(),
            StatusCode::BAD_REQUEST | StatusCode::TOO_MANY_REQUESTS
        ) {
            return downstream;
        }
        if !origin_allowed(&config, origin.as_deref()) {
            return (
                StatusCode::FORBIDDEN,
                Json(ApiError::new(
                    "cors_origin_denied",
                    "Origin is not allowed",
                    request_id,
                )),
            )
                .into_response();
        }
        let mut response = preflight_response(&config, origin.as_deref(), &request_id);
        copy_rate_headers(downstream.headers(), response.headers_mut());
        return response;
    }

    let mut response = next.run(request).await;
    apply_cors_headers(&config, origin.as_deref(), response.headers_mut());
    response
}

fn origin_allowed(config: &CorsConfig, origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true; // non-browser / same-origin without Origin header
    };
    config
        .allowed_origins
        .iter()
        .any(|allowed| allowed == origin)
}

fn preflight_response(config: &CorsConfig, origin: Option<&str>, request_id: &str) -> Response {
    let _ = request_id;
    let mut response = StatusCode::NO_CONTENT.into_response();
    apply_cors_headers(config, origin, response.headers_mut());
    if let Some(methods) = join_csv(&config.allowed_methods) {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_METHODS, methods);
    }
    if let Some(headers) = join_csv(&config.allowed_headers) {
        response
            .headers_mut()
            .insert(header::ACCESS_CONTROL_ALLOW_HEADERS, headers);
    }
    response.headers_mut().insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    response
}

fn apply_cors_headers(
    config: &CorsConfig,
    origin: Option<&str>,
    headers: &mut axum::http::HeaderMap,
) {
    // Always merge CORS Vary tokens when Origin is present (even if denied).
    if origin.is_some() {
        merge_vary(headers, VARY_CORS);
    }
    let Some(origin) = origin else {
        return;
    };
    if !origin_allowed(config, Some(origin)) {
        return;
    }
    if let Ok(value) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, value);
    }
    if config.allow_credentials {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }
    if let Some(expose) = join_csv(&config.expose_headers) {
        headers.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, expose);
    }
}

fn merge_vary(headers: &mut axum::http::HeaderMap, tokens: &[&str]) {
    let mut parts: Vec<String> = headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    for token in tokens {
        if !parts
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(token))
        {
            parts.push((*token).to_string());
        }
    }
    headers.remove(header::VARY);
    if let Ok(value) = HeaderValue::from_str(&parts.join(", ")) {
        headers.insert(header::VARY, value);
    }
}

fn join_csv(values: &[String]) -> Option<HeaderValue> {
    if values.is_empty() {
        return None;
    }
    HeaderValue::from_str(&values.join(", ")).ok()
}

fn copy_rate_headers(from: &axum::http::HeaderMap, to: &mut axum::http::HeaderMap) {
    for name in [
        "x-ratelimit-limit",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
    ] {
        if let Some(value) = from.get(name) {
            to.insert(name, value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderMap;

    use super::{merge_vary, origin_allowed};
    use crate::config::CorsConfig;

    #[test]
    fn exact_origin_match_only() {
        let config = CorsConfig {
            allowed_origins: vec!["https://app.example".into()],
            allowed_methods: vec!["GET".into()],
            allowed_headers: vec!["Authorization".into()],
            expose_headers: vec!["X-Request-Id".into()],
            allow_credentials: false,
        };
        assert!(origin_allowed(&config, Some("https://app.example")));
        assert!(!origin_allowed(&config, Some("https://evil.example")));
        assert!(origin_allowed(&config, None));
    }

    #[test]
    fn merge_vary_preserves_downstream_and_adds_cors_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::VARY, "Accept-Encoding".parse().unwrap());
        merge_vary(
            &mut headers,
            &[
                "Origin",
                "Access-Control-Request-Method",
                "Access-Control-Request-Headers",
            ],
        );
        let vary = headers
            .get(axum::http::header::VARY)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(vary.contains("Accept-Encoding"));
        assert!(vary.contains("Origin"));
        assert!(vary.contains("Access-Control-Request-Method"));
        assert!(vary.contains("Access-Control-Request-Headers"));
    }
}
