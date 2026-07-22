//! `POST /api/v1/ask` and `/api/v1/ask/stream` — grounded Q&A + closed SSE snapshot (P1B-R05).

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio::time::timeout;
use uuid::Uuid;

use crate::api::sse::{
    deliver_closed_snapshot, sse_response, DeliveryBounds, SSE_AUTH_PROBE_TIMEOUT,
};
use crate::api::{ApiRejection, AppJson};
use crate::auth::context::OrgContext;
use crate::auth::middleware::AuthenticatedOrg;
use crate::http::AppState;
use crate::routes::common::map_db;
use crate::routes::qa_common::{
    answer_to_json, build_auth_scope, default_snapshot_plan_bounds, event_row_to_envelope,
    exact_collection_ids, fresh_org_context, make_auth_probe, parse_ask_limit,
    parse_collection_ids, parse_query_text, parse_version_mode, plan_closed_events,
    probe_rejection, require_history_if_needed, require_query_perm, revalidate_stream_scope,
    run_hybrid_search, VersionModeBody,
};
use crate::services::qa::stream::{AuthProbeDecision, StreamCancel};
use crate::services::qa::{answer_question, QaRequest};
use crate::services::retrieval::{RetrievalRequest, RetrievalResponse, VersionMode};
use crate::services::sse_stream;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/ask", post(ask))
        .route("/api/v1/ask/stream", post(ask_stream))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskBody {
    question: String,
    #[serde(default)]
    collection_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    mode: Option<VersionModeBody>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    use_provider: Option<bool>,
}

struct PreparedAsk {
    ctx: OrgContext,
    mode: VersionMode,
    collection_ids: Vec<Uuid>,
    qa: QaRequest,
    retrieval: RetrievalResponse,
}

async fn prepare_ask(
    state: &AppState,
    auth: &AuthenticatedOrg,
    body: AskBody,
) -> Result<PreparedAsk, ApiRejection> {
    let request_id = auth.request_id.clone();
    let question = parse_query_text(&body.question, "question", &request_id)?;
    let requested = parse_collection_ids(body.collection_ids, &request_id)?;
    let mode = parse_version_mode(body.mode.as_ref(), &request_id)?;
    let limit = parse_ask_limit(body.limit, &request_id)?;
    let use_provider = body.use_provider.unwrap_or(true);

    let ctx = fresh_org_context(
        state,
        auth.context.org_id(),
        auth.context.user_id(),
        &request_id,
    )
    .await?;
    require_query_perm(&ctx, &request_id)?;
    require_history_if_needed(&ctx, &mode, &request_id)?;
    let collection_ids = exact_collection_ids(&ctx, requested.as_ref(), &request_id)?;

    let retrieval = run_hybrid_search(
        state,
        &ctx,
        RetrievalRequest {
            query: question.clone(),
            collection_ids: Some(collection_ids.iter().copied().collect()),
            mode: mode.clone(),
            limit,
            conflict_ids: vec![],
        },
        &request_id,
    )
    .await?;

    let qa = QaRequest {
        question,
        mode: mode.clone(),
        use_provider,
        conflict_lifecycle: vec![],
    };
    Ok(PreparedAsk {
        ctx,
        mode,
        collection_ids,
        qa,
        retrieval,
    })
}

async fn ask(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppJson(body): AppJson<AskBody>,
) -> Result<Json<serde_json::Value>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let prepared = prepare_ask(&state, &auth, body).await?;

    let provider = state.qa_provider();
    let provider_config = provider.map(|p| p.config());
    let answer = answer_question(prepared.qa, prepared.retrieval, provider, provider_config)
        .await
        .map_err(|error| {
            ApiRejection::validation(error.code(), &request_id)
                .with_details(json!({ "code": error.code() }))
        })?;

    Ok(Json(answer_to_json(&answer, &request_id)))
}

async fn ask_stream(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppJson(body): AppJson<AskBody>,
) -> Result<Response, ApiRejection> {
    let http_request_id = auth.request_id.clone();
    let prepared = prepare_ask(&state, &auth, body).await?;

    let provider = state.qa_provider();
    let provider_config = provider.map(|p| p.config());
    // R03 completes the whole answer before any SSE persist/delivery.
    let answer = answer_question(prepared.qa, prepared.retrieval, provider, provider_config)
        .await
        .map_err(|error| {
            ApiRejection::validation(error.code(), &http_request_id)
                .with_details(json!({ "code": error.code() }))
        })?;

    let auth_scope = build_auth_scope(&prepared.mode, prepared.collection_ids, &answer);
    // Probe immediately after QA/provider and before any sensitive persist.
    let mut probe = make_auth_probe(&state, auth.claims.clone(), auth_scope.clone());
    match timeout(SSE_AUTH_PROBE_TIMEOUT, probe()).await {
        Ok(AuthProbeDecision::Allow) => {}
        Ok(other) => return Err(probe_rejection(other, http_request_id)),
        Err(_) => return Err(probe_rejection(AuthProbeDecision::Deny, http_request_id)),
    }

    let plan_bounds = default_snapshot_plan_bounds();
    let (planned, close_reason) = plan_closed_events(&answer, plan_bounds);
    let stream_id = Uuid::new_v4();

    let (snapshot, events) = sse_stream::persist_ask_closed_snapshot(
        state.pool(),
        &prepared.ctx,
        sse_stream::PersistAskSnapshot {
            stream_id,
            auth_scope,
            planned,
            close_reason,
            max_events: plan_bounds.max_events,
            max_bytes: plan_bounds.max_bytes,
        },
    )
    .await
    .map_err(|error| map_db(error, &http_request_id))?;

    // Fresh revalidate before any payload delivery.
    let ctx = fresh_org_context(
        &state,
        auth.context.org_id(),
        auth.context.user_id(),
        &http_request_id,
    )
    .await?;
    revalidate_stream_scope(&ctx, &snapshot.auth_scope, &http_request_id)?;
    let mut probe = make_auth_probe(&state, auth.claims.clone(), snapshot.auth_scope.clone());
    match timeout(SSE_AUTH_PROBE_TIMEOUT, probe()).await {
        Ok(AuthProbeDecision::Allow) => {}
        Ok(other) => return Err(probe_rejection(other, http_request_id)),
        Err(_) => return Err(probe_rejection(AuthProbeDecision::Deny, http_request_id)),
    }

    let envelopes: Vec<_> = events
        .iter()
        .map(|row| event_row_to_envelope(row, &stream_id.to_string()))
        .collect();
    let cancel = StreamCancel::new();
    let delivery_probe = make_auth_probe(&state, auth.claims.clone(), snapshot.auth_scope.clone());
    let stream = deliver_closed_snapshot(
        envelopes,
        cancel,
        delivery_probe,
        DeliveryBounds::for_snapshot(snapshot.expires_at),
    );
    let mut response = sse_response(stream);
    response.headers_mut().insert(
        axum::http::HeaderName::from_static("x-request-id"),
        axum::http::HeaderValue::from_str(&stream_id.to_string())
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("invalid")),
    );
    Ok(response)
}
