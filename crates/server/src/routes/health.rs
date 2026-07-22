//! Liveness, readiness, and startup probes (P1B-R06).
//!
//! - `GET|HEAD /live` — process liveness (no dependency I/O)
//! - `GET|HEAD /ready` — required deps + signature + reconciliation (fail closed)
//! - `GET|HEAD /startup` — one-way startup completion / degraded
//!
//! Compatibility aliases remain at `/api/v1/health/{live,ready,startup}`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::api::ApiError;
use crate::http::AppState;
use crate::middleware::ResolvedRequestId;
pub use crate::services::health::*;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthBody {
    status: &'static str,
    request_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupBody {
    status: &'static str,
    completed: bool,
    degraded: bool,
    request_id: String,
}

fn live_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/live", get(liveness).head(liveness))
        .route("/ready", get(readiness).head(readiness))
        .route("/startup", get(startup).head(startup))
}

fn compat_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/health/live", get(liveness).head(liveness))
        .route("/api/v1/health/ready", get(readiness).head(readiness))
        .route("/api/v1/health/startup", get(startup).head(startup))
}

pub fn router() -> Router<Arc<AppState>> {
    live_routes().merge(compat_routes())
}

async fn liveness(ResolvedRequestId(request_id): ResolvedRequestId) -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        request_id,
    })
}

async fn startup(
    State(state): State<Arc<AppState>>,
    ResolvedRequestId(request_id): ResolvedRequestId,
) -> Response {
    let completed = state.startup_state().is_completed();
    let degraded = state.startup_state().is_degraded();
    if !completed {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(StartupBody {
                status: "starting",
                completed: false,
                degraded: false,
                request_id,
            }),
        )
            .into_response();
    }
    let status = if degraded { "degraded" } else { "ok" };
    (
        StatusCode::OK,
        Json(StartupBody {
            status,
            completed: true,
            degraded,
            request_id,
        }),
    )
        .into_response()
}

async fn readiness(
    State(state): State<Arc<AppState>>,
    ResolvedRequestId(request_id): ResolvedRequestId,
) -> Response {
    match state.check_readiness().await {
        Ok(()) => (
            StatusCode::OK,
            Json(HealthBody {
                status: "ok",
                request_id,
            }),
        )
            .into_response(),
        Err(reason) => {
            tracing::warn!(
                target: "readiness",
                reason = reason.as_str(),
                "dependency readiness check failed"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiError {
                    code: "dependency_unavailable".into(),
                    message: "A required service is unavailable".into(),
                    request_id,
                    details: None,
                }),
            )
                .into_response()
        }
    }
}
