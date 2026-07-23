//! Resumable job event SSE with live-tail (P1B-R05).
//!
//! Sequences are durable in `event_log`. Each pull takes family→principal→fresh
//! OrgContext→job/doc auth, selects at most one event under a fixed deadline,
//! commits/releases locks, then awaits the single-item client channel. Logout
//! and ACL writers therefore never block behind SSE send I/O.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures::stream;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::api::{resolve_last_event_id, ApiError, SseEnvelope};
use crate::auth::context::OrgContext;
use crate::auth::jwt::AccessClaims;
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::{resolve_org_context_on_txn, ResolveError};
use crate::auth::session::lock_refresh_family;
use crate::db::documents;
use crate::db::error::DbError;
use crate::db::jobs as job_repo;
use crate::db::models::{EventLogEntry, JobStatus};
use crate::db::pool::{apply_org_context, with_org_txn};
use crate::http::AppState;
use crate::services::access::{self, AccessError, PERMISSION_JOBS_SYSTEM};
use crate::services::authz_lock;
use crate::services::deletion::document_reads_suppressed;
use crate::services::qa::stream::SSE_ENVELOPE_VERSION;
use crate::services::stream_auth;

const SEND_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_IDLE: Duration = Duration::from_millis(500);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const PULL_DEADLINE: Duration = Duration::from_secs(2);
const CHANNEL_CAP: usize = 1;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/jobs/{job_id}/events", get(job_events))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(rename = "lastEventId")]
    last_event_id: Option<String>,
}

async fn job_events(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    client_ip: Option<axum::Extension<crate::middleware::ClientIp>>,
    Path(job_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>> + Send>, RouteError> {
    let ip = client_ip
        .map(|ext| ext.0 .0.clone())
        .unwrap_or_else(|| "unknown".into());
    crate::routes::rate_limit_guard::check_user(
        &state,
        &auth.context.org_id().to_string(),
        &auth.context.user_id().to_string(),
        &auth.request_id,
    )
    .map_err(RouteError::RateLimited)?;
    crate::routes::rate_limit_guard::check_route(&state, "jobs.events", &ip, &auth.request_id)
        .map_err(RouteError::RateLimited)?;

    let header = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok());
    // Syntax/conflict before authorize/provider side effects.
    let _cursor_syntax = resolve_last_event_id(query.last_event_id.as_deref(), header, None)
        .map_err(|error| RouteError::Validation(auth.request_id.clone(), error.message()))?;

    // Initial authorize via document collection ownership; IDOR → 404.
    let _job = access::resolve_job_access(state.pool(), &auth.context, job_id)
        .await
        .map_err(|error| match error {
            AccessError::NotFound => RouteError::NotFound(auth.request_id.clone()),
            _ => RouteError::Database(auth.request_id.clone()),
        })?;

    let high_water = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| Box::pin(async move { job_repo::job_event_high_water(txn, &ctx, job_id).await })
    })
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    let after = resolve_last_event_id(query.last_event_id.as_deref(), header, Some(high_water))
        .map_err(|error| RouteError::Validation(auth.request_id.clone(), error.message()))?;

    let request_id = auth.request_id.clone();
    let claims = auth.claims.clone();
    let pool = state.pool().clone();
    let (tx, rx) = mpsc::channel::<Event>(CHANNEL_CAP);

    tokio::spawn(async move {
        let mut cursor = after;
        let mut idle_polls = 0_u32;
        loop {
            if stream_auth::token_expired(&claims, chrono::Utc::now().timestamp()) {
                let _ = send_control_closed(&tx, &request_id, "token_expired").await;
                break;
            }

            // Reserve a single channel slot BEFORE any DB authorize/select.
            // With cap=1 this waits until the client drained the previous event,
            // so we never select/enqueue while a prior event is still buffered.
            // Reserve awaits the client channel with no DB locks held.
            let permit = match tokio::time::timeout(SEND_TIMEOUT, tx.reserve()).await {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => return,
                Err(_) => {
                    let _ = send_control_closed(
                        &tx,
                        &request_id,
                        stream_auth::StreamAuthError::SendTimeout.close_reason(),
                    )
                    .await;
                    return;
                }
            };

            let pull = tokio::time::timeout(PULL_DEADLINE, async {
                authorize_and_pull_one_job_event(&pool, &claims, job_id, cursor).await
            })
            .await;

            let (terminal, row) = match pull {
                Err(_) => {
                    drop(permit);
                    let _ = send_control_closed(&tx, &request_id, "stream_error").await;
                    break;
                }
                Ok(Err(error)) => {
                    drop(permit);
                    let _ = send_control_closed(&tx, &request_id, error.close_reason()).await;
                    break;
                }
                Ok(Ok(tuple)) => tuple,
            };

            let Some(row) = row else {
                drop(permit);
                idle_polls += 1;
                if terminal || idle_polls >= 30 {
                    let _ = send_control_closed(
                        &tx,
                        &request_id,
                        if terminal {
                            "snapshot_complete"
                        } else {
                            "live_tail_timeout"
                        },
                    )
                    .await;
                    break;
                }
                tokio::time::sleep(POLL_IDLE).await;
                continue;
            };
            idle_polls = 0;

            // Locks already released; enqueue is non-blocking via reserved permit.
            cursor = row.sequence_no;
            let envelope = SseEnvelope {
                version: SSE_ENVELOPE_VERSION,
                sequence: row.sequence_no as u64,
                event: row.event_type,
                request_id: request_id.clone(),
                data: json!({
                    "jobId": row.job_id,
                    "documentId": row.document_id,
                    "versionId": row.version_id,
                    "payload": row.payload,
                    "createdAt": row.created_at,
                }),
            };
            permit.send(sse_event(envelope));
        }
    });

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|event| (Ok::<_, Infallible>(event), rx))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(HEARTBEAT_INTERVAL)))
}

/// Family → principal → reload OrgContext → fresh job/doc auth → select ≤1 event.
async fn authorize_and_pull_one_job_event(
    pool: &deadpool_postgres::Pool,
    claims: &AccessClaims,
    job_id: Uuid,
    after_sequence: i64,
) -> Result<(bool, Option<EventLogEntry>), stream_auth::StreamAuthError> {
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| stream_auth::StreamAuthError::PrincipalDenied)?;
    let org_id = Uuid::parse_str(&claims.org_id)
        .map_err(|_| stream_auth::StreamAuthError::PrincipalDenied)?;
    let family_id =
        Uuid::parse_str(&claims.sid).map_err(|_| stream_auth::StreamAuthError::SessionRevoked)?;

    let provisional = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
        .map_err(|_| stream_auth::StreamAuthError::PrincipalDenied)?;

    let mut client = pool
        .get()
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;
    let txn = client
        .transaction()
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;
    apply_org_context(&txn, &provisional)
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;

    lock_refresh_family(&txn, family_id)
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;
    let family_live = txn
        .query_opt(
            "SELECT 1
             FROM refresh_tokens
             WHERE org_id = $1
               AND family_id = $2
               AND revoked_at IS NULL
               AND expires_at > now()
             LIMIT 1",
            &[&org_id, &family_id],
        )
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;
    if family_live.is_none() {
        let _ = txn.rollback().await;
        return Err(stream_auth::StreamAuthError::SessionRevoked);
    }

    authz_lock::lock_principal_authz(&txn, org_id, user_id)
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;
    let disabled: Option<chrono::DateTime<chrono::Utc>> = txn
        .query_one(
            "SELECT disabled_at FROM users WHERE id = $1 FOR SHARE",
            &[&user_id],
        )
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?
        .get(0);
    if disabled.is_some() {
        let _ = txn.rollback().await;
        return Err(stream_auth::StreamAuthError::PrincipalDenied);
    }
    let membership = txn
        .query_opt(
            "SELECT 1 FROM org_memberships
             WHERE org_id = $1 AND user_id = $2
             FOR SHARE",
            &[&org_id, &user_id],
        )
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;
    if membership.is_none() {
        let _ = txn.rollback().await;
        return Err(stream_auth::StreamAuthError::PrincipalDenied);
    }

    let ctx = resolve_org_context_on_txn(&txn, org_id, user_id)
        .await
        .map_err(|error| match error {
            ResolveError::UserDisabled | ResolveError::MembershipMissing => {
                stream_auth::StreamAuthError::PrincipalDenied
            }
            _ => stream_auth::StreamAuthError::Database,
        })?;

    let job = job_repo::get_by_id(&txn, &ctx, job_id)
        .await
        .map_err(|error| match error {
            DbError::NotFound => stream_auth::StreamAuthError::JobDenied,
            _ => stream_auth::StreamAuthError::Database,
        })?;
    match job.document_id {
        Some(document_id) => {
            let document = documents::get_by_id(&txn, &ctx, document_id)
                .await
                .map_err(|error| match error {
                    DbError::NotFound => stream_auth::StreamAuthError::JobDenied,
                    _ => stream_auth::StreamAuthError::Database,
                })?;
            if document_reads_suppressed(document.state, document.deleted_at.is_some())
                || !ctx.allows_collection(document.collection_id)
            {
                let _ = txn.rollback().await;
                return Err(stream_auth::StreamAuthError::JobDenied);
            }
        }
        None => {
            if !ctx.has_permission(PERMISSION_JOBS_SYSTEM) {
                let _ = txn.rollback().await;
                return Err(stream_auth::StreamAuthError::JobDenied);
            }
        }
    }

    let terminal = matches!(
        job.status,
        JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled | JobStatus::DeadLetter
    );
    let mut rows = job_repo::list_events_after(&txn, &ctx, after_sequence, Some(job_id), 1)
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;
    txn.commit()
        .await
        .map_err(|_| stream_auth::StreamAuthError::Database)?;
    Ok((terminal, rows.pop()))
}

async fn send_control_closed(
    tx: &mpsc::Sender<Event>,
    request_id: &str,
    reason: &str,
) -> Result<(), ()> {
    let data = json!({
        "version": SSE_ENVELOPE_VERSION,
        "event": "stream.closed",
        "requestId": request_id,
        "data": { "reason": reason },
        "control": true,
    });
    // No SSE id — must not advance Last-Event-ID.
    let event = Event::default()
        .event("stream.closed")
        .data(data.to_string());
    match tokio::time::timeout(Duration::from_millis(200), tx.send(event)).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

fn sse_event(envelope: SseEnvelope) -> Event {
    let data = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".into());
    Event::default()
        .id(envelope.sequence.to_string())
        .event(envelope.event)
        .data(data)
}

enum RouteError {
    NotFound(String),
    Database(String),
    Validation(String, &'static str),
    RateLimited(crate::routes::rate_limit_guard::RateLimitRejected),
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        if let Self::RateLimited(rejected) = self {
            return rejected.into_response();
        }
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
                "Events failed",
                request_id,
            ),
            Self::Validation(request_id, message) => (
                StatusCode::BAD_REQUEST,
                "validation_failed",
                message,
                request_id,
            ),
            Self::RateLimited(_) => unreachable!(),
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
