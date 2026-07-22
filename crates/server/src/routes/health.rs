//! Liveness/readiness endpoints (P1B-R06).
//!
//! Dependency probes live in `services::readiness` / `AppState` so this route
//! module stays free of direct storage product names (ADR 0001).

use std::sync::Arc;

use crate::api::ApiError;
use crate::http::AppState;
use crate::middleware::RequestId;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    status: &'static str,
    request_id: String,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/health/live", get(liveness))
        .route("/api/v1/health/ready", get(readiness_route))
}

async fn liveness(request_id: Option<axum::Extension<RequestId>>) -> Json<Health> {
    Json(Health {
        status: "ok",
        request_id: request_id
            .map(|id| id.0 .0)
            .unwrap_or_else(|| "missing-middleware-request-id".into()),
    })
}

async fn readiness_route(
    State(state): State<Arc<AppState>>,
    request_id: Option<axum::Extension<RequestId>>,
) -> Result<Json<Health>, ReadinessError> {
    let request_id = request_id
        .map(|id| id.0 .0)
        .unwrap_or_else(|| "missing-middleware-request-id".into());
    state
        .check_readiness()
        .await
        .map_err(|code| ReadinessError {
            code,
            request_id: request_id.clone(),
        })?;
    Ok(Json(Health {
        status: "ok",
        request_id,
    }))
}

struct ReadinessError {
    code: &'static str,
    request_id: String,
}

impl IntoResponse for ReadinessError {
    fn into_response(self) -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                code: "dependency_unavailable".into(),
                message: format!("A required service is unavailable ({})", self.code),
                request_id: self.request_id,
                details: Some(serde_json::json!({ "probe": self.code })),
            }),
        )
            .into_response()
    }
}
