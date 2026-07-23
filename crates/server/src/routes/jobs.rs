//! Job status API (P1B-R04).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use uuid::Uuid;

use crate::api::{ApiError, JobDto};
use crate::auth::middleware::AuthenticatedOrg;
use crate::http::AppState;
use crate::services::access::{self, AccessError};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/jobs/{job_id}", get(get_job))
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(job_id): Path<Uuid>,
) -> Result<Json<JobDto>, RouteError> {
    let job = access::resolve_job_access(state.pool(), &auth.context, job_id)
        .await
        .map_err(|error| match error {
            AccessError::NotFound => RouteError::NotFound(auth.request_id.clone()),
            _ => RouteError::Database(auth.request_id.clone()),
        })?;
    let request_id = crate::jobs::decode_job_payload(job.payload_version, job.payload.clone())
        .ok()
        .and_then(|payload| payload.request_id);
    Ok(Json(JobDto {
        id: job.id,
        job_type: job.job_type.as_str().into(),
        status: job.status.as_str().into(),
        attempts: job.attempts,
        document_id: job.document_id,
        version_id: job.version_id,
        request_id,
        created_at: job.created_at,
        updated_at: job.updated_at,
        finished_at: job.finished_at,
    }))
}

enum RouteError {
    NotFound(String),
    Database(String),
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        let (status, code, message, request_id) = match self {
            Self::NotFound(request_id) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Job not found",
                request_id,
            ),
            Self::Database(request_id) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Request failed",
                request_id,
            ),
        };
        (
            status,
            Json(ApiError {
                code: code.into(),
                message: message.into(),
                request_id,
                details: None,
            }),
        )
            .into_response()
    }
}
