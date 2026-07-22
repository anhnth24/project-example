//! `GET /api/v1/events/{request_id}` — resumable closed SSE snapshot replay (P1B-R05).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use uuid::Uuid;

use crate::api::sse::{
    deliver_closed_snapshot, last_event_id_from_headers, sse_response,
    validate_last_event_id_range, DeliveryBounds, SSE_AUTH_PROBE_TIMEOUT,
};
use crate::api::{ApiRejection, AppPath};
use crate::auth::middleware::AuthenticatedOrg;
use crate::db::pool::with_org_txn;
use crate::db::sse_streams::{self, ClosedSnapshotLoad, DEFAULT_CLEANUP_LIMIT};
use crate::http::AppState;
use crate::routes::common::{deny_or_not_found, map_db};
use crate::routes::qa_common::{
    event_row_to_envelope, fresh_org_context, make_auth_probe, probe_rejection,
    revalidate_stream_scope,
};
use crate::services::qa::stream::{AuthProbeDecision, StreamCancel};
use tokio::time::timeout;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/events/{request_id}", get(resume_events))
}

async fn resume_events(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(request_id): AppPath<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiRejection> {
    let http_request_id = auth.request_id.clone();
    let after = last_event_id_from_headers(&headers).map_err(|error| {
        ApiRejection::validation(error.message(), &http_request_id)
            .with_details(serde_json::json!({ "code": error.code(), "field": "Last-Event-ID" }))
    })?;
    let after_seq = after.unwrap_or(0);

    let ctx = fresh_org_context(
        &state,
        auth.context.org_id(),
        auth.context.user_id(),
        &http_request_id,
    )
    .await?;

    let loaded = with_org_txn(state.pool(), &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(
                async move { sse_streams::load_owned_closed_request(txn, &ctx, request_id).await },
            )
        }
    })
    .await
    .map_err(|error| match error {
        crate::db::error::DbError::NotFound => deny_or_not_found(&http_request_id),
        other => map_db(other, &http_request_id),
    })?;

    let stream_meta = match loaded {
        ClosedSnapshotLoad::Live(meta) => *meta,
        ClosedSnapshotLoad::Expired { request_id } => {
            // Deterministic 410 first; cleanup after (do not pre-delete before GET).
            let _ = with_org_txn(state.pool(), &ctx, {
                let ctx = ctx.clone();
                move |txn| {
                    Box::pin(
                        async move { sse_streams::expire_and_delete(txn, &ctx, request_id).await },
                    )
                }
            })
            .await;
            let _ = with_org_txn(state.pool(), &ctx, {
                let ctx = ctx.clone();
                move |txn| {
                    Box::pin(async move {
                        sse_streams::cleanup_expired(txn, &ctx, DEFAULT_CLEANUP_LIMIT, None).await
                    })
                }
            })
            .await;
            return Err(ApiRejection::new(
                StatusCode::GONE,
                "stream_expired",
                "Stream has expired",
                http_request_id,
            ));
        }
    };

    validate_last_event_id_range(after_seq, stream_meta.high_water_sequence()).map_err(
        |error| {
            ApiRejection::validation(error.message(), &http_request_id)
                .with_details(serde_json::json!({ "code": error.code(), "field": "Last-Event-ID" }))
        },
    )?;

    // Fresh permission + collection scope revalidation before any payload.
    revalidate_stream_scope(&ctx, &stream_meta.auth_scope, &http_request_id)?;
    let mut probe = make_auth_probe(
        state.clone(),
        auth.claims.clone(),
        stream_meta.auth_scope.clone(),
    );
    match timeout(SSE_AUTH_PROBE_TIMEOUT, probe()).await {
        Ok(AuthProbeDecision::Allow) => {}
        Ok(other) => return Err(probe_rejection(other, http_request_id)),
        Err(_) => return Err(probe_rejection(AuthProbeDecision::Deny, http_request_id)),
    }

    let events = with_org_txn(state.pool(), &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                sse_streams::list_events_after(txn, &ctx, request_id, after_seq).await
            })
        }
    })
    .await
    .map_err(|error| match error {
        crate::db::error::DbError::NotFound => deny_or_not_found(&http_request_id),
        other => map_db(other, &http_request_id),
    })?;

    // Bounded opportunistic cleanup; never remove the active request.
    let _ = with_org_txn(state.pool(), &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                sse_streams::cleanup_expired(txn, &ctx, DEFAULT_CLEANUP_LIMIT, Some(request_id))
                    .await
            })
        }
    })
    .await;

    let envelopes: Vec<_> = events
        .iter()
        .map(|row| event_row_to_envelope(row, &request_id.to_string()))
        .collect();
    let cancel = StreamCancel::new();
    let delivery_probe = make_auth_probe(
        state.clone(),
        auth.claims.clone(),
        stream_meta.auth_scope.clone(),
    );
    let stream = deliver_closed_snapshot(
        envelopes,
        cancel,
        delivery_probe,
        DeliveryBounds::for_snapshot(stream_meta.expires_at),
    );
    Ok(sse_response(stream))
}
