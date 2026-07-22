//! Resumable job event SSE with live-tail (P1B-R05).
//!
//! Sequences are durable in `event_log`. The stream replays after Last-Event-ID,
//! then live-tails while re-authorizing exp/session/membership/job each poll.

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

use crate::api::{ApiError, SseEnvelope};
use crate::auth::middleware::AuthenticatedOrg;
use crate::db::jobs as job_repo;
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::services::access::{self, AccessError};
use crate::services::qa::stream::{
    auth_closed_envelope, MAX_BUFFERED_EVENTS, SSE_ENVELOPE_VERSION,
};
use crate::services::stream_auth;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/jobs/{job_id}/events", get(job_events))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    #[serde(rename = "lastEventId")]
    last_event_id: Option<i64>,
}

async fn job_events(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(job_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>> + Send>, RouteError> {
    // Initial authorize via document collection ownership; IDOR → 404.
    let _job = access::resolve_job_access(state.pool(), &auth.context, job_id)
        .await
        .map_err(|error| match error {
            AccessError::NotFound => RouteError::NotFound(auth.request_id.clone()),
            _ => RouteError::Database(auth.request_id.clone()),
        })?;

    let after = query
        .last_event_id
        .or_else(|| {
            headers
                .get("last-event-id")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0);

    let request_id = auth.request_id.clone();
    let claims = auth.claims.clone();
    let pool = state.pool().clone();
    let (tx, rx) = mpsc::channel::<Event>(MAX_BUFFERED_EVENTS.min(64));

    tokio::spawn(async move {
        let mut cursor = after;
        let mut idle_polls = 0_u32;
        loop {
            let principal = match stream_auth::revalidate_job_stream(&pool, &claims, job_id).await {
                Ok(principal) => principal,
                Err(error) => {
                    let closed = auth_closed_envelope(
                        (cursor + 1) as u64,
                        &request_id,
                        error.close_reason(),
                    );
                    let _ = tx.send(sse_event(closed)).await;
                    break;
                }
            };
            let ctx = principal.context;
            let terminal = principal.terminal;

            let rows = match with_org_txn(&pool, &ctx, {
                let ctx = ctx.clone();
                move |txn| {
                    Box::pin(async move {
                        job_repo::list_events_after(txn, &ctx, cursor, Some(job_id), 100).await
                    })
                }
            })
            .await
            {
                Ok(rows) => rows,
                Err(_) => {
                    let closed =
                        auth_closed_envelope((cursor + 1) as u64, &request_id, "stream_error");
                    let _ = tx.send(sse_event(closed)).await;
                    break;
                }
            };

            if rows.is_empty() {
                idle_polls += 1;
                if terminal || idle_polls >= 30 {
                    let closed = auth_closed_envelope(
                        (cursor + 1) as u64,
                        &request_id,
                        if terminal {
                            "snapshot_complete"
                        } else {
                            "live_tail_timeout"
                        },
                    );
                    let _ = tx.send(sse_event(closed)).await;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            idle_polls = 0;
            for row in rows {
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
                if tx.send(sse_event(envelope)).await.is_err() {
                    // Slow/disconnected client — bounded channel drop closes stream.
                    return;
                }
            }
        }
    });

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|event| (Ok::<_, Infallible>(event), rx))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
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
                "Events failed",
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
