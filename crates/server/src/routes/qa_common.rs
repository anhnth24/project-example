//! Shared request mapping for search/ask routes (P1B-R05).

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

use crate::api::sse::{
    build_envelope, EVENT_CLOSE, EVENT_ERROR, EVENT_METADATA, EVENT_TOKEN, SSE_ENVELOPE_VERSION,
};
use crate::api::{ApiRejection, SseEnvelope};
use crate::auth::context::OrgContext;
use crate::auth::jwt::AccessClaims;
use crate::auth::permissions::{require_permission, resolve_org_context_in_txn};
use crate::auth::session;
use crate::db::models::DocumentState;
use crate::db::pool::with_org_txn;
use crate::db::sse_streams::{
    load_cited_document_pins, load_cited_version_pins, PlannedSseEvent, SseStreamEvent,
    StreamAuthScope, MAX_EVENT_PAYLOAD_BYTES, TERMINAL_EVENT_RESERVE,
};
use crate::http::AppState;
use crate::routes::common::map_resolve;
use crate::services::deletion::document_reads_suppressed;
use crate::services::qa::stream::{
    tokenize_for_stream, AuthProbeDecision, DEFAULT_MAX_STREAM_BYTES, DEFAULT_MAX_STREAM_TOKENS,
};
use crate::services::qa::{QaAnswer, QaCitation};
use crate::services::retrieval::{
    hybrid_search, resolve_scope, RetrievalError, RetrievalHit, RetrievalRequest,
    RetrievalResponse, VersionMode, PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY,
};

pub const MAX_QUERY_CHARS: usize = 4_000;
pub const MAX_COLLECTION_FILTER: usize = 64;
pub const DEFAULT_SEARCH_LIMIT: usize = 8;
pub const MAX_SEARCH_LIMIT: usize = 100;
pub const MAX_ASK_LIMIT: usize = 32;

/// R03-equivalent + migration hard caps applied before any DB write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotPlanBounds {
    pub max_events: i32,
    pub max_bytes: i64,
    pub max_event_payload_bytes: i32,
    pub max_token_events: usize,
    pub max_token_bytes: usize,
}

impl Default for SnapshotPlanBounds {
    fn default() -> Self {
        Self {
            max_events: crate::db::sse_streams::DEFAULT_MAX_EVENTS,
            max_bytes: crate::db::sse_streams::DEFAULT_MAX_BYTES,
            max_event_payload_bytes: MAX_EVENT_PAYLOAD_BYTES,
            max_token_events: DEFAULT_MAX_STREAM_TOKENS,
            max_token_bytes: DEFAULT_MAX_STREAM_BYTES,
        }
    }
}

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

pub async fn run_hybrid_search(
    state: &AppState,
    ctx: &OrgContext,
    request: RetrievalRequest,
    request_id: &str,
) -> Result<RetrievalResponse, ApiRejection> {
    let qdrant = state.qdrant().ok_or_else(|| {
        ApiRejection::new(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "dependency_unavailable",
            "Search dependency is unavailable",
            request_id,
        )
    })?;
    hybrid_search(state.pool(), qdrant, state.embedder(), ctx, request)
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
    json!({
        "citeId": citation.cite_id,
        "documentId": citation.document_id,
        "versionId": citation.version_id,
        "versionNumber": citation.version_number,
        "contentSha256": citation.content_sha256,
        "chunkId": citation.chunk_id,
        "isCurrent": citation.is_current,
        "heading": citation.heading,
        "quote": citation.quote
    })
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

pub fn metadata_data(answer: &QaAnswer) -> JsonValue {
    json!({
        "mode": answer.mode.as_str(),
        "grounded": answer.grounded,
        "citationCount": answer.citations.len(),
        "citations": answer.citations.iter().map(citation_to_json).collect::<Vec<_>>(),
        "warnings": answer.warnings,
        "versionContext": {
            "mode": answer.version_context.mode,
            "currentVersionIds": answer.version_context.current_version_ids,
            "citedVersionIds": answer.version_context.cited_version_ids,
            "changeNote": answer.version_context.change_note
        },
        "answerMode": answer.mode.as_str(),
        "fallbackReason": answer.audit.fallback_reason,
        "envelopeVersion": SSE_ENVELOPE_VERSION
    })
}

fn metadata_data_slim(answer: &QaAnswer) -> JsonValue {
    json!({
        "mode": answer.mode.as_str(),
        "grounded": answer.grounded,
        "citationCount": answer.citations.len(),
        "warnings": answer.warnings,
        "versionContext": {
            "mode": answer.version_context.mode,
            "currentVersionIds": answer.version_context.current_version_ids,
            "citedVersionIds": answer.version_context.cited_version_ids,
            "changeNote": answer.version_context.change_note
        },
        "answerMode": answer.mode.as_str(),
        "fallbackReason": answer.audit.fallback_reason,
        "envelopeVersion": SSE_ENVELOPE_VERSION,
        "citationsTruncated": true
    })
}

pub fn version_mode_label(mode: &VersionMode) -> &'static str {
    match mode {
        VersionMode::Current => "current",
        VersionMode::AsOf { .. } => "as_of",
        VersionMode::Compare { .. } => "compare",
        VersionMode::History { .. } => "history",
    }
}

pub fn mode_requires_history(mode: &VersionMode) -> bool {
    !matches!(mode, VersionMode::Current)
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

pub fn build_auth_scope(
    mode: &VersionMode,
    collection_ids: Vec<Uuid>,
    answer: &QaAnswer,
) -> StreamAuthScope {
    let mut docs = BTreeSet::new();
    let mut versions = BTreeSet::new();
    for citation in &answer.citations {
        docs.insert(citation.document_id);
        versions.insert(citation.version_id);
    }
    for id in &answer.version_context.cited_version_ids {
        versions.insert(*id);
    }
    StreamAuthScope {
        version_mode: version_mode_label(mode).to_string(),
        requires_history: mode_requires_history(mode),
        collection_ids,
        cited_document_ids: docs.into_iter().collect(),
        cited_version_ids: versions.into_iter().collect(),
    }
}

fn json_payload_bytes(value: &JsonValue) -> i32 {
    i32::try_from(serde_json::to_vec(value).unwrap_or_default().len()).unwrap_or(i32::MAX)
}

fn fits_event(data: &JsonValue, bounds: SnapshotPlanBounds) -> bool {
    let bytes = json_payload_bytes(data);
    bytes >= 0 && bytes <= bounds.max_event_payload_bytes
}

fn safe_truncated_snapshot() -> (Vec<PlannedSseEvent>, &'static str) {
    (
        vec![PlannedSseEvent {
            event_type: EVENT_ERROR,
            data: json!({ "reason": "truncated" }),
        }],
        "truncated",
    )
}

/// Build contiguous planned events with R03-equivalent hard caps before DB.
///
/// Always returns a persistable snapshot (terminal `truncated` or `completed`).
/// Never panics; oversized metadata yields a safe one-event error snapshot.
pub fn plan_closed_events(
    answer: &QaAnswer,
    bounds: SnapshotPlanBounds,
) -> (Vec<PlannedSseEvent>, &'static str) {
    if bounds.max_events <= TERMINAL_EVENT_RESERVE
        || bounds.max_bytes <= 0
        || bounds.max_event_payload_bytes <= 0
        || bounds.max_event_payload_bytes > MAX_EVENT_PAYLOAD_BYTES
        || bounds.max_token_events == 0
        || bounds.max_token_bytes == 0
    {
        return safe_truncated_snapshot();
    }

    let capacity = (bounds.max_events - TERMINAL_EVENT_RESERVE).max(1) as usize;
    let mut events = Vec::new();

    let metadata = {
        let full = metadata_data(answer);
        if fits_event(&full, bounds) && i64::from(json_payload_bytes(&full)) <= bounds.max_bytes {
            full
        } else {
            let slim = metadata_data_slim(answer);
            if fits_event(&slim, bounds) && i64::from(json_payload_bytes(&slim)) <= bounds.max_bytes
            {
                slim
            } else {
                return safe_truncated_snapshot();
            }
        }
    };
    let mut byte_count = i64::from(json_payload_bytes(&metadata));
    events.push(PlannedSseEvent {
        event_type: EVENT_METADATA,
        data: metadata,
    });

    let tokens = tokenize_for_stream(&answer.answer);
    let mut truncated = false;
    let mut token_events = 0usize;
    let mut token_bytes = 0usize;
    for token in tokens {
        if token.is_empty() {
            continue;
        }
        if events.len() >= capacity
            || token_events >= bounds.max_token_events
            || token_bytes.saturating_add(token.len()) > bounds.max_token_bytes
        {
            truncated = true;
            break;
        }
        let data = json!({ "text": token });
        if !fits_event(&data, bounds) {
            truncated = true;
            break;
        }
        let payload = i64::from(json_payload_bytes(&data));
        // Reserve room for a small terminal event.
        let terminal_reserve = 64i64;
        if byte_count
            .saturating_add(payload)
            .saturating_add(terminal_reserve)
            > bounds.max_bytes
        {
            truncated = true;
            break;
        }
        byte_count = byte_count.saturating_add(payload);
        token_bytes = token_bytes.saturating_add(token.len());
        token_events = token_events.saturating_add(1);
        events.push(PlannedSseEvent {
            event_type: EVENT_TOKEN,
            data,
        });
    }

    let (terminal_type, reason) = if truncated {
        (EVENT_ERROR, "truncated")
    } else {
        (EVENT_CLOSE, "completed")
    };
    let terminal_data = json!({ "reason": reason });
    if !fits_event(&terminal_data, bounds)
        || byte_count.saturating_add(i64::from(json_payload_bytes(&terminal_data)))
            > bounds.max_bytes
        || events.len() as i32 >= bounds.max_events
    {
        return safe_truncated_snapshot();
    }
    events.push(PlannedSseEvent {
        event_type: terminal_type,
        data: terminal_data,
    });

    // Final validation: every payload within migration + total caps.
    let mut total = 0i64;
    for event in &events {
        let bytes = json_payload_bytes(&event.data);
        if bytes > bounds.max_event_payload_bytes {
            return safe_truncated_snapshot();
        }
        total = total.saturating_add(i64::from(bytes));
        if total > bounds.max_bytes {
            return safe_truncated_snapshot();
        }
    }
    if events.len() as i32 > bounds.max_events {
        return safe_truncated_snapshot();
    }

    (events, reason)
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

/// Revalidate cited document/version pins against current membership/ACL/state.
///
/// Missing / tombstoned / `deleted_at` → Deleted. Not indexed, unpublished,
/// wrong lineage, or collection ACL → Deny. Empty pins → Allow.
pub async fn probe_cited_pins(
    state: &AppState,
    ctx: &OrgContext,
    scope: &StreamAuthScope,
) -> AuthProbeDecision {
    if scope.cited_document_ids.is_empty() && scope.cited_version_ids.is_empty() {
        return AuthProbeDecision::Allow;
    }
    let ctx = ctx.clone();
    let scope = scope.clone();
    match with_org_txn(state.pool(), &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                if !scope.cited_document_ids.is_empty() {
                    let rows =
                        load_cited_document_pins(txn, &ctx, &scope.cited_document_ids).await?;
                    if rows.len() != scope.cited_document_ids.len() {
                        return Ok(AuthProbeDecision::Deleted);
                    }
                    for row in &rows {
                        let Ok(state) = DocumentState::parse(&row.state) else {
                            return Ok(AuthProbeDecision::Deny);
                        };
                        if document_reads_suppressed(state, row.deleted_at.is_some()) {
                            return Ok(AuthProbeDecision::Deleted);
                        }
                        if state != DocumentState::Indexed {
                            return Ok(AuthProbeDecision::Deny);
                        }
                        if !ctx.allows_collection(row.collection_id) {
                            return Ok(AuthProbeDecision::Deny);
                        }
                    }
                }
                if !scope.cited_version_ids.is_empty() {
                    let rows = load_cited_version_pins(txn, &ctx, &scope.cited_version_ids).await?;
                    if rows.len() != scope.cited_version_ids.len() {
                        return Ok(AuthProbeDecision::Deleted);
                    }
                    let cited_docs: BTreeSet<Uuid> =
                        scope.cited_document_ids.iter().copied().collect();
                    for row in &rows {
                        let Ok(state) = DocumentState::parse(&row.document_state) else {
                            return Ok(AuthProbeDecision::Deny);
                        };
                        if document_reads_suppressed(state, row.deleted_at.is_some()) {
                            return Ok(AuthProbeDecision::Deleted);
                        }
                        if state != DocumentState::Indexed {
                            return Ok(AuthProbeDecision::Deny);
                        }
                        if row.publication_state != "published" {
                            return Ok(AuthProbeDecision::Deny);
                        }
                        if !cited_docs.is_empty() && !cited_docs.contains(&row.document_id) {
                            return Ok(AuthProbeDecision::Deny);
                        }
                        if !ctx.allows_collection(row.collection_id) {
                            return Ok(AuthProbeDecision::Deny);
                        }
                    }
                }
                Ok(AuthProbeDecision::Allow)
            })
        }
    })
    .await
    {
        Ok(decision) => decision,
        Err(_) => AuthProbeDecision::Deny,
    }
}

/// Fresh auth + session-family + permission/collection/cited-pin probe.
pub fn make_auth_probe(
    state: Arc<AppState>,
    claims: AccessClaims,
    scope: StreamAuthScope,
) -> impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = AuthProbeDecision> + Send>>
       + Send
       + 'static {
    move || {
        let state = state.clone();
        let claims = claims.clone();
        let scope = scope.clone();
        Box::pin(async move {
            let now = Utc::now().timestamp();
            if claims.exp <= now {
                return AuthProbeDecision::Deny;
            }
            let Ok(user_id) = Uuid::parse_str(&claims.sub) else {
                return AuthProbeDecision::Deny;
            };
            let Ok(org_id) = Uuid::parse_str(&claims.org_id) else {
                return AuthProbeDecision::Deny;
            };
            let Ok(family_id) = Uuid::parse_str(&claims.sid) else {
                return AuthProbeDecision::Deny;
            };
            let Ok(ctx) = resolve_org_context_in_txn(state.pool(), org_id, user_id).await else {
                return AuthProbeDecision::Deny;
            };
            if require_permission(&ctx, PERMISSION_QA_QUERY).is_err() {
                return AuthProbeDecision::Deny;
            }
            if scope.requires_history && require_permission(&ctx, PERMISSION_QA_HISTORY).is_err() {
                return AuthProbeDecision::Deny;
            }
            if scope
                .collection_ids
                .iter()
                .any(|id| !ctx.allows_collection(*id))
            {
                return AuthProbeDecision::Deny;
            }
            match session::is_refresh_family_active(state.pool(), org_id, family_id).await {
                Ok(true) => {}
                Ok(false) | Err(_) => return AuthProbeDecision::Deny,
            }
            // Deny/Deleted before each event for cited pins.
            probe_cited_pins(&state, &ctx, &scope).await
        })
    }
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
    if mode_requires_history(mode) {
        crate::routes::common::require_perm(ctx, PERMISSION_QA_HISTORY, request_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::qa::grounding::VersionContext;
    use crate::services::qa::{AnswerMode, QaAuditMetadata};

    fn sample_answer(answer: &str, quote: &str) -> QaAnswer {
        let doc = Uuid::new_v4();
        let version = Uuid::new_v4();
        QaAnswer {
            answer: answer.to_string(),
            citations: vec![QaCitation {
                cite_id: "c1".into(),
                document_id: doc,
                version_id: version,
                version_number: 1,
                content_sha256: "a".repeat(64),
                chunk_id: Uuid::new_v4(),
                is_current: true,
                heading: "H".into(),
                quote: quote.to_string(),
            }],
            mode: AnswerMode::OfflineExtractive,
            grounded: true,
            warnings: vec![],
            version_context: VersionContext {
                mode: "current",
                current_version_ids: vec![version],
                cited_version_ids: vec![version],
                change_note: None,
            },
            conflict_warnings: vec![],
            audit: QaAuditMetadata {
                action: "ask",
                outcome: "ok",
                answer_mode: AnswerMode::OfflineExtractive.as_str(),
                citation_count: 1,
                conflict_warning_count: 0,
                version_mode: "current",
                provider_configured: false,
                fallback_reason: None,
                request_id: "test".into(),
                grounded: true,
                latency_ms: 1,
                error: None,
            },
        }
    }

    #[test]
    fn oversized_metadata_is_bounded_or_truncated() {
        let huge = "x".repeat(70_000);
        let answer = sample_answer("short", &huge);
        let (events, reason) = plan_closed_events(&answer, SnapshotPlanBounds::default());
        // Full citations exceed migration payload; planner slims or emits truncated.
        assert!(matches!(reason, "completed" | "truncated"));
        assert!(!events.is_empty());
        assert!(events
            .iter()
            .all(|e| json_payload_bytes(&e.data) <= MAX_EVENT_PAYLOAD_BYTES));
        if reason == "completed" {
            assert_eq!(events[0].event_type, EVENT_METADATA);
            assert_eq!(events[0].data["citationsTruncated"], true);
        } else {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event_type, EVENT_ERROR);
            assert_eq!(events[0].data["reason"], "truncated");
        }

        // Hard total-byte cap forces safe truncated error snapshot (never 500).
        let tiny = SnapshotPlanBounds {
            max_events: 8,
            max_bytes: 32,
            max_event_payload_bytes: MAX_EVENT_PAYLOAD_BYTES,
            max_token_events: DEFAULT_MAX_STREAM_TOKENS,
            max_token_bytes: DEFAULT_MAX_STREAM_BYTES,
        };
        let (events, reason) = plan_closed_events(&answer, tiny);
        assert_eq!(reason, "truncated");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EVENT_ERROR);
        assert!(json_payload_bytes(&events[0].data) <= MAX_EVENT_PAYLOAD_BYTES);
    }

    #[test]
    fn token_caps_truncate_deterministically() {
        let answer = sample_answer(&"word ".repeat(5_000), "q");
        let bounds = SnapshotPlanBounds {
            max_events: 8,
            max_bytes: 256 * 1024,
            max_event_payload_bytes: MAX_EVENT_PAYLOAD_BYTES,
            max_token_events: 3,
            max_token_bytes: DEFAULT_MAX_STREAM_BYTES,
        };
        let (events, reason) = plan_closed_events(&answer, bounds);
        assert_eq!(reason, "truncated");
        assert_eq!(events.last().unwrap().event_type, EVENT_ERROR);
        assert_eq!(events.last().unwrap().data["reason"], "truncated");
        assert!(events.len() <= 8);
        let token_count = events
            .iter()
            .filter(|e| e.event_type == EVENT_TOKEN)
            .count();
        assert_eq!(token_count, 3);
    }
}
