//! Hybrid search REST API (P1B-R05).

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::error::DbError;
use crate::db::models::AuditOutcome;
use crate::db::projects;
use crate::http::AppState;
use crate::services::audit;
use crate::services::citation::pins_from_hits;
use crate::services::retrieval::{
    hybrid_search, RetrievalError, RetrievalRequest, VersionMode, PERMISSION_QA_QUERY,
};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/search", post(search))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchBody {
    query: String,
    #[serde(default)]
    collection_ids: Option<Vec<Uuid>>,
    /// P2-18 — optional project scope. `None` (the default) is "all
    /// projects": no additional filter beyond whatever `collectionIds`
    /// already restricts, i.e. today's exact behavior. Resolved to the
    /// project's collection ids server-side and intersected with
    /// `collectionIds` (if both are given) before being handed to the
    /// existing `resolve_scope`/ACL-intersection retrieval path — no new
    /// retrieval mechanism, see `routes::search`/`routes::ask` module notes.
    ///
    /// Deprecated (P2-19): kept working exactly as-is (the web app still
    /// sends it) — prefer `projectIds` below, which subsumes it. Sending
    /// both unions them (`db::projects::merge_project_ids`).
    #[serde(default)]
    project_id: Option<Uuid>,
    /// P2-19 — union of project scopes. Same narrow-never-widen contract as
    /// `projectId` above, generalized to many ids: resolves to the *union*
    /// of every named project's collection ids, intersected with
    /// `collectionIds`. Any unresolvable id (wrong org, never created) 404s
    /// the whole request — consistent with the singular field. Bounded to
    /// `db::projects::MAX_PROJECT_IDS`; a longer array is a 400. An absent
    /// or empty array is "no filter from this field", identical to omitting
    /// `projectId` — it only narrows scope when non-empty.
    #[serde(default)]
    project_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    as_of: Option<DateTime<Utc>>,
    #[serde(default)]
    document_id: Option<Uuid>,
    #[serde(default)]
    version_a: Option<Uuid>,
    #[serde(default)]
    version_b: Option<Uuid>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    conflict_ids: Vec<Uuid>,
}

fn default_limit() -> usize {
    10
}

fn parse_mode(body: &SearchBody) -> Result<VersionMode, &'static str> {
    match body.mode.as_deref().unwrap_or("current") {
        "current" => Ok(VersionMode::Current),
        "as_of" => {
            let at = body.as_of.ok_or("as_of requires asOf timestamp")?;
            Ok(VersionMode::AsOf { at })
        }
        "compare" => Ok(VersionMode::Compare {
            document_id: body.document_id.ok_or("compare requires documentId")?,
            version_a: body.version_a.ok_or("compare requires versionA")?,
            version_b: body.version_b.ok_or("compare requires versionB")?,
        }),
        "history" => Ok(VersionMode::History {
            document_id: body.document_id.ok_or("history requires documentId")?,
        }),
        _ => Err("mode must be current|as_of|compare|history"),
    }
}

async fn search(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    client_ip: Option<axum::Extension<crate::middleware::ClientIp>>,
    Json(body): Json<SearchBody>,
) -> Result<Json<serde_json::Value>, RouteError> {
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
    crate::routes::rate_limit_guard::check_route(&state, "search", &ip, &auth.request_id)
        .map_err(RouteError::RateLimited)?;
    if require_permission(&auth.context, PERMISSION_QA_QUERY).is_err() {
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "search.query",
            "search",
            None,
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    if body.query.trim().is_empty() || body.query.len() > 8_192 {
        audit::record(
            state.pool(),
            &auth.context,
            audit::AuditRecord {
                request_id: &auth.request_id,
                action: "search.query",
                resource_type: "search",
                resource_id: None,
                outcome: AuditOutcome::Error,
                metadata: serde_json::json!({ "reason": "validation_failed" }),
            },
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid query",
        ));
    }
    let mode = parse_mode(&body)
        .map_err(|message| RouteError::Validation(auth.request_id.clone(), message))?;
    // P2-19 — merge/bound before any database round trip; a too-long
    // `projectIds` array is a plain 400, same class of check as `parse_mode`
    // just above.
    let project_ids = projects::merge_project_ids(body.project_id, body.project_ids)
        .map_err(|message| RouteError::Validation(auth.request_id.clone(), message))?;
    let collection_ids = body
        .collection_ids
        .map(|ids| ids.into_iter().collect::<BTreeSet<_>>());
    // P2-18/P2-19 — narrow (never widen) the requested collection scope to
    // the union of the named projects' collections, still ANDed with the
    // caller's ACL by the existing `resolve_scope` inside `hybrid_search`
    // below. Resolved before the `vector_index` availability check right
    // below: an unknown project id is a client input error independent of
    // retrieval backend health, so it must 404 even when the vector store is
    // unavailable.
    let collection_ids =
        projects::resolve_project_scope(state.pool(), &auth.context, &project_ids, collection_ids)
            .await
            .map_err(|error| match error {
                DbError::NotFound => RouteError::NotFound(auth.request_id.clone()),
                _ => RouteError::Database(auth.request_id.clone()),
            })?;
    let vector_index = state
        .vector_index()
        .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
    let query_chars = body.query.len();
    let limit = body.limit.clamp(1, 100);
    let response = match hybrid_search(
        state.pool(),
        vector_index,
        state.embedder(),
        &auth.context,
        RetrievalRequest {
            query: body.query,
            collection_ids,
            mode,
            limit,
            conflict_ids: body.conflict_ids,
        },
    )
    .await
    {
        Ok(response) => response,
        Err(RetrievalError::PermissionDenied | RetrievalError::EmptyScope) => {
            audit::record_deny(
                state.pool(),
                &auth.context,
                &auth.request_id,
                "search.query",
                "search",
                None,
                "permission_denied",
            )
            .await
            .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
            return Err(RouteError::Denied(auth.request_id.clone()));
        }
        Err(error) => return Err(RouteError::from_retrieval(error, &auth.request_id)),
    };
    let citations = pins_from_hits(auth.context.org_id(), &response.hits);
    audit::record(
        state.pool(),
        &auth.context,
        audit::AuditRecord {
            request_id: &auth.request_id,
            action: "search.query",
            resource_type: "search",
            resource_id: None,
            outcome: AuditOutcome::Success,
            metadata: serde_json::json!({
                "hit_count": response.hits.len(),
                "query_chars": query_chars,
                "limit": limit as i64,
            }),
        },
    )
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    let hits: Vec<serde_json::Value> = response
        .hits
        .iter()
        .map(|hit| {
            serde_json::json!({
                "chunkId": hit.chunk_id,
                "collectionId": hit.collection_id,
                "documentId": hit.document_id,
                "versionId": hit.version_id,
                "versionNumber": hit.version_number,
                "sourceContentSha256": hit.content_sha256,
                "canonicalMarkdownSha256": hit.canonical_markdown_sha256,
                "heading": hit.heading,
                "snippet": hit.snippet,
                "lexicalScore": hit.lexical_score,
                "vectorScore": hit.vector_score,
                "rerankScore": hit.rerank_score,
                "isCurrent": hit.is_current,
                "effectiveFrom": hit.effective_from,
                "effectiveTo": hit.effective_to,
                "page": hit.page,
                "slide": hit.slide,
                "sheet": hit.sheet,
                "spanStart": hit.span_start,
                "spanEnd": hit.span_end,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "hits": hits,
        "citations": citations,
        "warnings": response.warnings,
        "embeddingMode": response.embedding_mode,
        "conflictEvidence": response.conflict_evidence.iter().map(|item| {
            serde_json::json!({
                "conflictId": item.conflict_id,
                "status": item.status,
                "resolutionNote": item.resolution_note,
                "resolvedAt": item.resolved_at,
                "claimAId": item.claim_a_id,
                "claimBId": item.claim_b_id,
            })
        }).collect::<Vec<_>>(),
        "requestId": auth.request_id,
    })))
}

enum RouteError {
    Validation(String, &'static str),
    Denied(String),
    NotFound(String),
    Unavailable(String),
    Database(String),
    RateLimited(crate::routes::rate_limit_guard::RateLimitRejected),
}

impl RouteError {
    fn from_retrieval(error: RetrievalError, request_id: &str) -> Self {
        match error {
            RetrievalError::PermissionDenied | RetrievalError::EmptyScope => {
                Self::Denied(request_id.to_string())
            }
            RetrievalError::InvalidRequest(_) | RetrievalError::LineageMismatch => {
                Self::Validation(request_id.to_string(), "Invalid search request")
            }
            _ => Self::Database(request_id.to_string()),
        }
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        if let Self::RateLimited(rejected) = self {
            return rejected.into_response();
        }
        let (status, code, message, request_id) = match self {
            Self::Validation(request_id, message) => (
                StatusCode::BAD_REQUEST,
                "validation_failed",
                message,
                request_id,
            ),
            Self::Denied(request_id) => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "Permission denied",
                request_id,
            ),
            Self::NotFound(request_id) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Project not found",
                request_id,
            ),
            Self::Unavailable(request_id) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "dependency_unavailable",
                "Search dependencies unavailable",
                request_id,
            ),
            Self::Database(request_id) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Search failed",
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
