//! `GET /api/v1/events/{request_id}` — resumable closed SSE snapshot replay (P1B-R05).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use tokio::time::timeout;
use uuid::Uuid;

use crate::api::sse::{
    deliver_closed_snapshot, last_event_id_from_headers, sse_response,
    validate_last_event_id_range, DeliveryBounds, SSE_AUTH_PROBE_TIMEOUT,
};
use crate::api::{ApiRejection, AppPath};
use crate::auth::middleware::AuthenticatedOrg;
use crate::http::AppState;
use crate::routes::common::{deny_or_not_found, map_db};
use crate::routes::qa_common::{
    event_row_to_envelope, fresh_org_context, make_auth_probe, probe_rejection,
    revalidate_stream_scope,
};
use crate::services::qa::stream::{AuthProbeDecision, StreamCancel};
use crate::services::sse_stream::{self, ClosedSnapshotLoad};

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

    let loaded = sse_stream::load_owned_closed_snapshot(state.pool(), &ctx, request_id)
        .await
        .map_err(|error| match error {
            crate::db::error::DbError::NotFound => deny_or_not_found(&http_request_id),
            other => map_db(other, &http_request_id),
        })?;

    let stream_meta = match loaded {
        ClosedSnapshotLoad::Live(meta) => *meta,
        ClosedSnapshotLoad::Expired { request_id } => {
            // Deterministic 410 first; cleanup after (do not pre-delete before GET).
            let _ = sse_stream::expire_stream_and_cleanup(state.pool(), &ctx, request_id).await;
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
    let mut probe = make_auth_probe(&state, auth.claims.clone(), stream_meta.auth_scope.clone());
    match timeout(SSE_AUTH_PROBE_TIMEOUT, probe()).await {
        Ok(AuthProbeDecision::Allow) => {}
        Ok(other) => return Err(probe_rejection(other, http_request_id)),
        Err(_) => return Err(probe_rejection(AuthProbeDecision::Deny, http_request_id)),
    }

    let events =
        sse_stream::list_events_after_and_cleanup(state.pool(), &ctx, request_id, after_seq)
            .await
            .map_err(|error| match error {
                crate::db::error::DbError::NotFound => deny_or_not_found(&http_request_id),
                other => map_db(other, &http_request_id),
            })?;

    let envelopes: Vec<_> = events
        .iter()
        .map(|row| event_row_to_envelope(row, &request_id.to_string()))
        .collect();
    let cancel = StreamCancel::new();
    let delivery_probe =
        make_auth_probe(&state, auth.claims.clone(), stream_meta.auth_scope.clone());
    let stream = deliver_closed_snapshot(
        envelopes,
        cancel,
        delivery_probe,
        DeliveryBounds::for_snapshot(stream_meta.expires_at),
    );
    Ok(sse_response(stream))
}
