use axum::body::Body;
use axum::http::header::HeaderName;
use axum::http::{HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

pub(crate) static X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Debug, Clone)]
pub(crate) struct RequestId(pub(crate) String);

pub async fn ensure_request_id(mut request: Request<Body>, next: Next) -> Response {
    let request_id = Uuid::new_v4().to_string();
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    let mut response = next.run(request).await;
    if !response.headers().contains_key(&X_REQUEST_ID) {
        response.headers_mut().insert(
            X_REQUEST_ID.clone(),
            HeaderValue::from_str(&request_id).expect("UUID is a valid header value"),
        );
    }
    response
}
