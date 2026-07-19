//! Job status REST routes.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::middleware::AuthenticatedOrg;
use crate::db::models::Job;
use crate::db::pool::with_org_txn_typed;
use crate::db::{documents, jobs};
use crate::http::AppState;
use crate::routes::common::{
    parse_uuid, require_collection_or_404, require_permission_or_403, RestError, TxnRestError,
};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/jobs/{jobId}", get(get_job))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobPath {
    job_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobResponse {
    id: Uuid,
    job_type: &'static str,
    status: &'static str,
    attempts: i32,
    max_attempts: i32,
    document_id: Option<Uuid>,
    version_id: Option<Uuid>,
    available_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<Job> for JobResponse {
    fn from(value: Job) -> Self {
        Self {
            id: value.id,
            job_type: value.job_type.as_str(),
            status: value.status.as_str(),
            attempts: value.attempts,
            max_attempts: value.max_attempts,
            document_id: value.document_id,
            version_id: value.version_id,
            available_at: value.available_at,
            started_at: value.started_at,
            finished_at: value.finished_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<JobPath>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let job_id = parse_uuid(&path.job_id, &request_id)?;
    let job = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_permission_or_403(&ctx, "qa.query", &request_id)?;
                let job = jobs::get_by_id(txn, &ctx, job_id)
                    .await?
                    .ok_or_else(|| RestError::not_found(&request_id))?;
                let document_id = job
                    .document_id
                    .ok_or_else(|| RestError::not_found(&request_id))?;
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                require_collection_or_404(&ctx, document.collection_id, &request_id)?;
                Ok::<_, TxnRestError>(job)
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;
    Ok(Json(JobResponse::from(job)).into_response())
}
