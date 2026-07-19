//! SSE event routes for status streams.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use futures::stream::{self, BoxStream};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::sse::{envelope, event_from_envelope};
use crate::auth::context::OrgContext;
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::{require_permission, resolve_org_context_in_txn};
use crate::db::models::{Job, JobStatus};
use crate::db::pool::with_org_txn_typed;
use crate::db::{documents, jobs};
use crate::http::AppState;
use crate::routes::common::{
    parse_uuid, require_collection_or_404, require_permission_or_403, RestError, TxnRestError,
};
use crate::routes::jobs::JobResponse;

const JOB_POLL_INTERVAL: Duration = Duration::from_secs(1);
const JOB_EVENTS_MAX_DURATION: Duration = Duration::from_secs(10 * 60);

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/jobs/{jobId}/events", get(job_events))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JobEventsPath {
    job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JobSnapshotKey {
    status: &'static str,
    attempts: i32,
    updated_at: chrono::DateTime<Utc>,
}

async fn job_events(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<JobEventsPath>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let job_id = parse_uuid(&path.job_id, &request_id)?;
    let initial = load_authorized_job(state.clone(), &auth.context, job_id, &request_id).await?;
    let stream = job_event_stream(state, auth, job_id, initial);
    Ok(Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response())
}

fn job_event_stream(
    state: Arc<AppState>,
    auth: AuthenticatedOrg,
    job_id: Uuid,
    initial: Job,
) -> BoxStream<'static, Result<Event, Infallible>> {
    struct State {
        app: Arc<AppState>,
        auth: AuthenticatedOrg,
        job_id: Uuid,
        request_id: String,
        stream_id: Uuid,
        sequence: u64,
        last: Option<JobSnapshotKey>,
        initial: Option<Job>,
        interval: tokio::time::Interval,
        started_at: Instant,
        closed: bool,
    }

    let mut interval = tokio::time::interval(JOB_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    stream::unfold(
        State {
            app: state,
            request_id: auth.request_id.clone(),
            auth,
            job_id,
            stream_id: Uuid::new_v4(),
            sequence: 0,
            last: None,
            initial: Some(initial),
            interval,
            started_at: Instant::now(),
            closed: false,
        },
        |mut state| async move {
            loop {
                if state.closed {
                    return None;
                }

                let job = if let Some(job) = state.initial.take() {
                    job
                } else {
                    state.interval.tick().await;
                    if state.started_at.elapsed() > JOB_EVENTS_MAX_DURATION {
                        state.closed = true;
                        state.sequence += 1;
                        let envelope = envelope(
                            state.sequence,
                            "job.close",
                            &state.request_id,
                            json!({ "reason": "max_duration" }),
                        );
                        return Some((event_from_envelope(state.stream_id, &envelope), state));
                    }
                    match refresh_and_load_authorized_job(
                        state.app.clone(),
                        &state.auth,
                        state.job_id,
                        &state.request_id,
                    )
                    .await
                    {
                        Ok(job) => job,
                        Err(()) => {
                            state.closed = true;
                            state.sequence += 1;
                            let envelope = envelope(
                                state.sequence,
                                "job.close",
                                &state.request_id,
                                json!({ "reason": "authorization_changed" }),
                            );
                            return Some((event_from_envelope(state.stream_id, &envelope), state));
                        }
                    }
                };

                let key = snapshot_key(&job);
                if state.last.as_ref() == Some(&key) {
                    continue;
                }

                state.last = Some(key);
                state.sequence += 1;
                let event_name = if is_terminal(job.status) {
                    state.closed = true;
                    "job.done"
                } else {
                    "job.status"
                };
                let envelope = envelope(
                    state.sequence,
                    event_name,
                    &state.request_id,
                    serde_json::to_value(JobResponse::from(job))
                        .expect("job response is serializable"),
                );
                return Some((event_from_envelope(state.stream_id, &envelope), state));
            }
        },
    )
    .boxed()
}

async fn refresh_and_load_authorized_job(
    state: Arc<AppState>,
    auth: &AuthenticatedOrg,
    job_id: Uuid,
    request_id: &str,
) -> Result<Job, ()> {
    if auth.claims.exp <= Utc::now().timestamp() {
        return Err(());
    }
    let org_id = auth.context.org_id();
    let user_id = auth.context.user_id();
    let fresh = resolve_org_context_in_txn(state.pool(), org_id, user_id)
        .await
        .map_err(|_| ())?;
    require_permission(&fresh, "qa.query").map_err(|_| ())?;
    load_authorized_job(state, &fresh, job_id, request_id)
        .await
        .map_err(|_| ())
}

async fn load_authorized_job(
    state: Arc<AppState>,
    ctx: &OrgContext,
    job_id: Uuid,
    request_id: &str,
) -> Result<Job, RestError> {
    with_org_txn_typed(state.pool(), ctx, {
        let ctx = ctx.clone();
        let request_id = request_id.to_string();
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
    .map_err(|error: TxnRestError| error.into_rest(request_id))
}

fn snapshot_key(job: &Job) -> JobSnapshotKey {
    JobSnapshotKey {
        status: job.status.as_str(),
        attempts: job.attempts,
        updated_at: job.updated_at,
    }
}

fn is_terminal(status: JobStatus) -> bool {
    matches!(
        status,
        JobStatus::Succeeded | JobStatus::DeadLetter | JobStatus::Cancelled | JobStatus::Failed
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_jobs_are_terminal_for_sse() {
        assert!(is_terminal(JobStatus::Succeeded));
        assert!(is_terminal(JobStatus::DeadLetter));
        assert!(is_terminal(JobStatus::Cancelled));
        assert!(is_terminal(JobStatus::Failed));
        assert!(!is_terminal(JobStatus::Pending));
    }
}
