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
    build_auth_scope, default_snapshot_plan_bounds, plan_closed_events, SnapshotPlanBounds,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::sse::{EVENT_ERROR, EVENT_METADATA, EVENT_TOKEN};
    use crate::db::sse_streams::MAX_EVENT_PAYLOAD_BYTES;
    use crate::services::qa::grounding::VersionContext;
    use crate::services::qa::stream::{DEFAULT_MAX_STREAM_BYTES, DEFAULT_MAX_STREAM_TOKENS};
    use crate::services::qa::{AnswerMode, QaAuditMetadata};
    use crate::services::sse_stream::json_payload_bytes;

    fn mode_body(mode_type: &str) -> VersionModeBody {
        VersionModeBody {
            mode_type: mode_type.to_string(),
            at: None,
            document_id: None,
            version_a: None,
            version_b: None,
        }
    }

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
    fn version_mode_parser_covers_supported_shapes_and_required_fields() {
        assert_eq!(
            parse_version_mode(None, "request").unwrap(),
            VersionMode::Current
        );

        let at = Utc::now();
        let mut as_of = mode_body("asOf");
        as_of.at = Some(at);
        assert_eq!(
            parse_version_mode(Some(&as_of), "request").unwrap(),
            VersionMode::AsOf { at }
        );

        let document_id = Uuid::new_v4();
        let version_a = Uuid::new_v4();
        let version_b = Uuid::new_v4();
        let mut compare = mode_body("compare");
        compare.document_id = Some(document_id);
        compare.version_a = Some(version_a);
        compare.version_b = Some(version_b);
        assert_eq!(
            parse_version_mode(Some(&compare), "request").unwrap(),
            VersionMode::Compare {
                document_id,
                version_a,
                version_b,
            }
        );

        let mut history = mode_body("history");
        history.document_id = Some(document_id);
        assert_eq!(
            parse_version_mode(Some(&history), "request").unwrap(),
            VersionMode::History { document_id }
        );

        for invalid in [
            mode_body("as_of"),
            mode_body("compare"),
            mode_body("history"),
            mode_body("future"),
        ] {
            assert!(parse_version_mode(Some(&invalid), "request").is_err());
        }
    }

    #[test]
    fn request_bounds_trim_unicode_and_reject_invalid_limits() {
        assert_eq!(
            parse_query_text("  đối soát  ", "query", "request").unwrap(),
            "đối soát"
        );
        assert!(parse_query_text(" \n\t ", "query", "request").is_err());
        assert!(parse_query_text(&"ấ".repeat(MAX_QUERY_CHARS), "query", "request").is_ok());
        assert!(parse_query_text(&"ấ".repeat(MAX_QUERY_CHARS + 1), "query", "request").is_err());

        assert_eq!(
            parse_search_limit(None, "request").unwrap(),
            DEFAULT_SEARCH_LIMIT
        );
        assert_eq!(
            parse_search_limit(Some(MAX_SEARCH_LIMIT as u32), "request").unwrap(),
            MAX_SEARCH_LIMIT
        );
        assert!(parse_search_limit(Some(0), "request").is_err());
        assert!(parse_search_limit(Some((MAX_SEARCH_LIMIT + 1) as u32), "request").is_err());

        assert_eq!(
            parse_ask_limit(Some(MAX_ASK_LIMIT as u32), "request").unwrap(),
            MAX_ASK_LIMIT
        );
        assert!(parse_ask_limit(Some(0), "request").is_err());
        assert!(parse_ask_limit(Some((MAX_ASK_LIMIT + 1) as u32), "request").is_err());
    }

    #[test]
    fn collection_filter_is_bounded_and_deduplicated() {
        assert_eq!(parse_collection_ids(None, "request").unwrap(), None);
        assert!(parse_collection_ids(Some(vec![]), "request").is_err());

        let collection = Uuid::new_v4();
        let parsed = parse_collection_ids(Some(vec![collection, collection]), "request").unwrap();
        assert_eq!(parsed.unwrap(), BTreeSet::from([collection]));

        let too_many = (0..=MAX_COLLECTION_FILTER)
            .map(|_| Uuid::new_v4())
            .collect();
        assert!(parse_collection_ids(Some(too_many), "request").is_err());
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
