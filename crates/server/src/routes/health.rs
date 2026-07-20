use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use uuid::Uuid;

use crate::api::ApiError;
use crate::http::AppState;
use crate::services::health::{check_dependencies, HealthErrorKind};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    status: &'static str,
    request_id: String,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/health/live", get(liveness))
        .route("/api/v1/health/ready", get(readiness))
        .route("/api/v1/health/start", get(startup))
}

async fn liveness() -> Json<Health> {
    Json(healthy())
}

async fn startup(State(state): State<Arc<AppState>>) -> Result<Json<Health>, HealthError> {
    if state.startup_complete() {
        Ok(Json(healthy()))
    } else {
        Err(HealthError::dependency_unavailable())
    }
}

async fn readiness(State(state): State<Arc<AppState>>) -> Result<Json<Health>, HealthError> {
    check_dependencies(state).await.map_err(HealthError::from)?;
    Ok(Json(healthy()))
}

fn healthy() -> Health {
    Health {
        status: "ok",
        request_id: Uuid::new_v4().to_string(),
    }
}

struct HealthError {
    code: &'static str,
    message: &'static str,
}

impl HealthError {
    fn dependency_unavailable() -> Self {
        Self {
            code: "dependency_unavailable",
            message: "A required service is unavailable",
        }
    }
}

impl From<HealthErrorKind> for HealthError {
    fn from(value: HealthErrorKind) -> Self {
        match value {
            HealthErrorKind::DependencyUnavailable => Self::dependency_unavailable(),
            HealthErrorKind::ConfigurationInvalid => Self {
                code: "configuration_invalid",
                message: "Server configuration is invalid",
            },
            HealthErrorKind::NotReconciled => Self {
                code: "not_reconciled",
                message: "Restore or reconciliation is still in progress",
            },
        }
    }
}

impl IntoResponse for HealthError {
    fn into_response(self) -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                code: self.code.into(),
                message: self.message.into(),
                request_id: Uuid::new_v4().to_string(),
                details: None,
            }),
        )
            .into_response()
    }
}
