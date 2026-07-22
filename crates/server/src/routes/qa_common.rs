//! Shared HTTP parse/map helpers for search/ask/SSE routes (P1B-R05).
//!
//! Routes stay free of vector-store/DB wiring (ADR 0001): this module maps
//! request/response shapes and delegates use cases to `services::*`.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

use crate::api::sse::build_envelope;
use crate::api::{ApiRejection, SseEnvelope};
use crate::auth::context::OrgContext;
use crate::auth::jwt::AccessClaims;
use crate::auth::permissions::resolve_org_context_in_txn;
use crate::http::AppState;
use crate::routes::common::map_resolve;
use crate::services::qa::stream::AuthProbeDecision;
use crate::services::qa::{QaAnswer, QaCitation};
use crate::services::retrieval::{
    hybrid_search_with_backends, resolve_scope, RetrievalError, RetrievalHit, RetrievalRequest,
    RetrievalResponse, VersionMode, PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY,
};
use crate::services::sse_stream::{
    self, citation_to_json as citation_json, SseStreamEvent, StreamAuthScope,
};

pub const MAX_QUERY_CHARS: usize = 4_000;
pub const MAX_COLLECTION_FILTER: usize = 64;
pub const DEFAULT_SEARCH_LIMIT: usize = 8;
pub const MAX_SEARCH_LIMIT: usize = 100;
pub const MAX_ASK_LIMIT: usize = 32;

pub use crate::services::sse_stream::{
    build_auth_scope, default_snapshot_plan_bounds, plan_closed_events,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionModeBody {
    #[serde(rename = "type")]
    pub mode_type: String,
    #[serde(default)]
    pub at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub document_id: Option<Uuid>,
    #[serde(default)]
    pub version_a: Option<Uuid>,
    #[serde(default)]
    pub version_b: Option<Uuid>,
}

pub fn parse_version_mode(
    body: Option<&VersionModeBody>,
    request_id: &str,
) -> Result<VersionMode, ApiRejection> {
    let Some(body) = body else {
        return Ok(VersionMode::Current);
    };
    match body.mode_type.as_str() {
        "current" => Ok(VersionMode::Current),
        "as_of" | "asOf" => {
            let at = body.at.ok_or_else(|| {
                ApiRejection::validation("as_of mode requires at", request_id)
                    .with_details(json!({ "field": "mode.at" }))
            })?;
            Ok(VersionMode::AsOf { at })
        }
        "compare" => {
            let document_id = body.document_id.ok_or_else(|| {
                ApiRejection::validation("compare mode requires documentId", request_id)
                    .with_details(json!({ "field": "mode.documentId" }))
            })?;
            let version_a = body.version_a.ok_or_else(|| {
                ApiRejection::validation("compare mode requires versionA", request_id)
                    .with_details(json!({ "field": "mode.versionA" }))
            })?;
            let version_b = body.version_b.ok_or_else(|| {
                ApiRejection::validation("compare mode requires versionB", request_id)
                    .with_details(json!({ "field": "mode.versionB" }))
            })?;
            Ok(VersionMode::Compare {
                document_id,
                version_a,
                version_b,
            })
        }
        "history" => {
            let document_id = body.document_id.ok_or_else(|| {
                ApiRejection::validation("history mode requires documentId", request_id)
                    .with_details(json!({ "field": "mode.documentId" }))
            })?;
            Ok(VersionMode::History { document_id })
        }
        _ => Err(ApiRejection::validation(
            "mode.type must be current, as_of, compare, or history",
            request_id,
        )
        .with_details(json!({ "field": "mode.type" }))),
    }
}

pub fn parse_collection_ids(
    raw: Option<Vec<Uuid>>,
    request_id: &str,
) -> Result<Option<BTreeSet<Uuid>>, ApiRejection> {
    let Some(ids) = raw else {
        return Ok(None);
    };
    if ids.is_empty() {
        return Err(ApiRejection::validation(
            "collectionIds must not be empty when provided",
            request_id,
        ));
    }
    if ids.len() > MAX_COLLECTION_FILTER {
        return Err(ApiRejection::validation(
            "too many collectionIds",
            request_id,
        ));
    }
    Ok(Some(ids.into_iter().collect()))
}

pub fn parse_search_limit(limit: Option<u32>, request_id: &str) -> Result<usize, ApiRejection> {
    let limit = limit.map(|v| v as usize).unwrap_or(DEFAULT_SEARCH_LIMIT);
    if !(1..=MAX_SEARCH_LIMIT).contains(&limit) {
        return Err(
            ApiRejection::validation("limit must be between 1 and 100", request_id)
                .with_details(json!({ "field": "limit" })),
        );
    }
    Ok(limit)
}

pub fn parse_ask_limit(limit: Option<u32>, request_id: &str) -> Result<usize, ApiRejection> {
    let limit = limit
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_SEARCH_LIMIT.min(MAX_ASK_LIMIT));
    if !(1..=MAX_ASK_LIMIT).contains(&limit) {
        return Err(
            ApiRejection::validation("limit must be between 1 and 32", request_id)
                .with_details(json!({ "field": "limit" })),
        );
    }
    Ok(limit)
}

pub fn parse_query_text(text: &str, field: &str, request_id: &str) -> Result<String, ApiRejection> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(
            ApiRejection::validation(format!("{field} must not be empty"), request_id)
                .with_details(json!({ "field": field })),
        );
    }
    if trimmed.chars().count() > MAX_QUERY_CHARS {
        return Err(
            ApiRejection::validation(format!("{field} is too long"), request_id)
                .with_details(json!({ "field": field })),
        );
    }
    Ok(trimmed.to_string())
}

pub async fn fresh_org_context(
    state: &AppState,
    org_id: Uuid,
    user_id: Uuid,
    request_id: &str,
) -> Result<OrgContext, ApiRejection> {
    resolve_org_context_in_txn(state.pool(), org_id, user_id)
        .await
        .map_err(|error| map_resolve(error, request_id))
}

/// Route-facing hybrid search: backend wiring lives in the retrieval service.
pub async fn run_hybrid_search(
    state: &AppState,
    ctx: &OrgContext,
    request: RetrievalRequest,
    request_id: &str,
) -> Result<RetrievalResponse, ApiRejection> {
    hybrid_search_with_backends(
        state.pool(),
        state.vector_store(),
        state.embedder(),
        ctx,
        request,
    )
    .await
    .map_err(|error| map_retrieval_error(error, request_id))
}

pub fn map_retrieval_error(error: RetrievalError, request_id: &str) -> ApiRejection {
    match error {
        RetrievalError::PermissionDenied => ApiRejection::permission_denied(request_id),
        RetrievalError::EmptyScope => ApiRejection::collection_denied(request_id),
        RetrievalError::InvalidRequest(message) => ApiRejection::validation(message, request_id),
        RetrievalError::LineageMismatch => ApiRejection::validation(
            "compare/history versions are not in one lineage",
            request_id,
        ),
        RetrievalError::DependencyUnavailable => ApiRejection::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "dependency_unavailable",
            "Search dependency is unavailable",
            request_id,
        ),
        RetrievalError::Database(_)
        | RetrievalError::Storage(_)
        | RetrievalError::Embedding(_)
        | RetrievalError::BothLegsFailed => ApiRejection::internal(request_id),
    }
}

pub fn hit_to_json(hit: &RetrievalHit) -> JsonValue {
    json!({
        "chunkId": hit.chunk_id,
        "collectionId": hit.collection_id,
        "documentId": hit.document_id,
        "versionId": hit.version_id,
        "versionNumber": hit.version_number,
        "contentSha256": hit.content_sha256,
        "heading": hit.heading,
        "snippet": hit.snippet,
        "score": hit.rerank_score,
        "isCurrent": hit.is_current,
        "locator": {
            "page": hit.page,
            "slide": hit.slide,
            "sheet": hit.sheet,
            "spanStart": hit.span_start,
            "spanEnd": hit.span_end
        }
    })
}

pub fn citation_to_json(citation: &QaCitation) -> JsonValue {
    citation_json(citation)
}

pub fn answer_to_json(answer: &QaAnswer, request_id: &str) -> JsonValue {
    json!({
        "answer": answer.answer,
        "citations": answer.citations.iter().map(citation_to_json).collect::<Vec<_>>(),
        "mode": answer.mode.as_str(),
        "grounded": answer.grounded,
        "warnings": answer.warnings,
        "versionContext": {
            "mode": answer.version_context.mode,
            "currentVersionIds": answer.version_context.current_version_ids,
            "citedVersionIds": answer.version_context.cited_version_ids,
            "changeNote": answer.version_context.change_note
        },
        "conflictWarnings": answer.conflict_warnings.iter().map(|w| json!({
            "status": w.status.as_str(),
            "message": w.message,
            "pinCiteIds": w.pin_cite_ids
        })).collect::<Vec<_>>(),
        "requestId": request_id
    })
}

/// Exact collection IDs authorized for this retrieval (resolved allow-list intersection).
pub fn exact_collection_ids(
    ctx: &OrgContext,
    requested: Option<&BTreeSet<Uuid>>,
    request_id: &str,
) -> Result<Vec<Uuid>, ApiRejection> {
    resolve_scope(ctx, requested)
        .map(|scope| scope.collection_ids.into_iter().collect())
        .map_err(|error| map_retrieval_error(error, request_id))
}

pub fn event_row_to_envelope(row: &SseStreamEvent, request_id: &str) -> SseEnvelope {
    build_envelope(
        u64::try_from(row.sequence_no).unwrap_or(0),
        row.event_type.clone(),
        request_id.to_string(),
        row.data.clone(),
    )
}

pub fn revalidate_stream_scope(
    ctx: &OrgContext,
    scope: &StreamAuthScope,
    request_id: &str,
) -> Result<(), ApiRejection> {
    require_query_perm(ctx, request_id)?;
    if scope.requires_history {
        crate::routes::common::require_perm(ctx, PERMISSION_QA_HISTORY, request_id)?;
    }
    for collection_id in &scope.collection_ids {
        if !ctx.allows_collection(*collection_id) {
            return Err(ApiRejection::collection_denied(request_id));
        }
    }
    Ok(())
}

/// Map pre-delivery probe failures to HTTP rejections.
pub fn probe_rejection(decision: AuthProbeDecision, request_id: impl Into<String>) -> ApiRejection {
    let request_id = request_id.into();
    match decision {
        AuthProbeDecision::Allow => ApiRejection::internal(&request_id),
        AuthProbeDecision::Deny => ApiRejection::new(
            axum::http::StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Session is no longer valid",
            request_id,
        ),
        AuthProbeDecision::Deleted => crate::routes::common::deny_or_not_found(&request_id),
    }
}

/// Route helper: build delivery auth probe via the SSE stream service.
pub fn make_auth_probe(
    state: &AppState,
    claims: AccessClaims,
    scope: StreamAuthScope,
) -> impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = AuthProbeDecision> + Send>>
       + Send
       + 'static {
    sse_stream::make_auth_probe(state.pool().clone(), claims, scope)
}

pub fn require_query_perm(ctx: &OrgContext, request_id: &str) -> Result<(), ApiRejection> {
    crate::routes::common::require_perm(ctx, PERMISSION_QA_QUERY, request_id)?;
    Ok(())
}

pub fn require_history_if_needed(
    ctx: &OrgContext,
    mode: &VersionMode,
    request_id: &str,
) -> Result<(), ApiRejection> {
    if sse_stream::mode_requires_history(mode) {
        crate::routes::common::require_perm(ctx, PERMISSION_QA_HISTORY, request_id)?;
    }
    Ok(())
}
