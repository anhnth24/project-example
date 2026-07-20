//! Search API routes backed by the tenant-scoped retrieval engine.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::middleware::AuthenticatedOrg;
use crate::http::AppState;
use crate::routes::common::{require_permission_or_403, RestError};
use crate::services::audit::{self, SafeAuditEvent};
use crate::services::retrieval::{
    retrieve, Degradation, GroundedHit, RetrievalError, RetrievalRequest, VersionMode,
    MAX_RETRIEVAL_LIMIT,
};

pub(crate) const JSON_BODY_LIMIT: usize = 16 * 1024;
pub(crate) const MAX_QUERY_CHARS: usize = 4096;
pub(crate) const MAX_COLLECTION_IDS: usize = 512;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/search", post(search))
        .route_layer(DefaultBodyLimit::max(JSON_BODY_LIMIT))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchRequestBody {
    pub(crate) query: String,
    pub(crate) limit: Option<u32>,
    pub(crate) collection_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    hits: Vec<SearchHitResponse>,
    degraded: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchHitResponse {
    chunk_id: Uuid,
    document_id: Uuid,
    version_id: Uuid,
    collection_id: Uuid,
    version_number: i32,
    snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    heading_path: Option<Vec<String>>,
    lexical_score: f32,
    vector_score: f32,
    rerank_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slide: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sheet: Option<String>,
    is_current: bool,
}

pub(crate) struct ValidSearchRequest {
    pub(crate) query: String,
    pub(crate) limit: usize,
    pub(crate) ctx: OrgContext,
}

async fn search(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    body: Result<Json<SearchRequestBody>, JsonRejection>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let Json(body) =
        body.map_err(|_| RestError::validation("request body is invalid", &request_id))?;
    if let Err(error) = require_permission_or_403(&auth.context, "qa.query", &request_id) {
        warn_audit_failure(
            audit_qa_query(
                &state,
                &auth.context,
                "deny",
                &request_id,
                serde_json::json!({ "endpoint": "search", "reason": "permission_denied" }),
            )
            .await,
            "deny",
            &request_id,
        );
        return Err(error);
    }
    let input = validate_search_request(body, &auth.context, &request_id)?;
    let audit_metadata = serde_json::json!({
        "endpoint": "search",
        "limit": input.limit,
        "collectionScopeCount": input.ctx.allowed_collection_ids().len()
    });
    let retrieval_started = Instant::now();
    let response = match retrieve(
        state.pool(),
        state.vector_store(),
        &input.ctx,
        RetrievalRequest {
            query: input.query,
            limit: input.limit,
            mode: VersionMode::Current,
        },
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            state.metrics().observe_retrieval_latency(
                "search",
                "error",
                retrieval_started.elapsed().as_secs_f64(),
            );
            let (outcome, reason) = match &error {
                RetrievalError::EmptyScope => ("deny", "empty_scope"),
                _ => ("error", "retrieval_failed"),
            };
            warn_audit_failure(
                audit_qa_query(
                    &state,
                    &auth.context,
                    outcome,
                    &request_id,
                    merge_audit_reason(audit_metadata, reason),
                )
                .await,
                outcome,
                &request_id,
            );
            return Err(map_retrieval_error(error, &request_id));
        }
    };
    state.metrics().observe_retrieval_latency(
        "search",
        "success",
        retrieval_started.elapsed().as_secs_f64(),
    );
    warn_audit_failure(
        audit_qa_query(
            &state,
            &auth.context,
            "success",
            &request_id,
            audit_metadata,
        )
        .await,
        "success",
        &request_id,
    );

    Ok(Json(SearchResponse {
        hits: response
            .hits
            .into_iter()
            .map(SearchHitResponse::from)
            .collect(),
        degraded: response.degraded.map(degradation_code),
    })
    .into_response())
}

async fn audit_qa_query(
    state: &Arc<AppState>,
    ctx: &OrgContext,
    outcome: &'static str,
    request_id: &str,
    metadata: serde_json::Value,
) -> Result<(), crate::db::error::DbError> {
    audit::record_audit_event(
        state.pool(),
        ctx,
        SafeAuditEvent {
            action: "qa.query",
            resource_type: "qa",
            resource_id: None,
            outcome,
            request_id: request_id.into(),
            metadata,
        },
    )
    .await
}

fn warn_audit_failure(
    result: Result<(), crate::db::error::DbError>,
    outcome: &'static str,
    request_id: &str,
) {
    if let Err(error) = result {
        tracing::warn!(
            action = "qa.query",
            outcome = outcome,
            request_id = %request_id,
            error_code = error.code(),
            "audit write failed"
        );
    }
}

fn merge_audit_reason(mut metadata: serde_json::Value, reason: &'static str) -> serde_json::Value {
    if let Some(object) = metadata.as_object_mut() {
        object.insert("reason".into(), serde_json::Value::String(reason.into()));
    }
    metadata
}

pub(crate) fn validate_search_request(
    body: SearchRequestBody,
    ctx: &OrgContext,
    request_id: &str,
) -> Result<ValidSearchRequest, RestError> {
    let query = body.query.trim().to_string();
    if query.is_empty() {
        return Err(RestError::validation("query must not be empty", request_id));
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(RestError::validation("query is too long", request_id));
    }
    validate_collection_ids_len(body.collection_ids.as_ref(), request_id)?;
    let limit = validate_limit(body.limit, MAX_RETRIEVAL_LIMIT, request_id)?;
    Ok(ValidSearchRequest {
        query,
        limit,
        ctx: narrowed_context(ctx, body.collection_ids),
    })
}

pub(crate) fn validate_collection_ids_len(
    collection_ids: Option<&Vec<Uuid>>,
    request_id: &str,
) -> Result<(), RestError> {
    if collection_ids.is_some_and(|ids| ids.len() > MAX_COLLECTION_IDS) {
        return Err(RestError::validation(
            format!("collectionIds must contain at most {MAX_COLLECTION_IDS} entries"),
            request_id,
        ));
    }
    Ok(())
}

pub(crate) fn validate_limit(
    limit: Option<u32>,
    max: usize,
    request_id: &str,
) -> Result<usize, RestError> {
    let limit = limit.unwrap_or(max.min(10) as u32);
    let limit = usize::try_from(limit)
        .map_err(|_| RestError::validation("limit is invalid", request_id))?;
    if !(1..=max).contains(&limit) {
        return Err(RestError::validation(
            format!("limit must be between 1 and {max}"),
            request_id,
        ));
    }
    Ok(limit)
}

pub(crate) fn narrowed_context(
    ctx: &OrgContext,
    requested_collection_ids: Option<Vec<Uuid>>,
) -> OrgContext {
    match requested_collection_ids {
        Some(collection_ids) => ctx.with_narrowed_collections(collection_ids),
        None => ctx.clone(),
    }
}

pub(crate) fn map_retrieval_error(error: RetrievalError, request_id: &str) -> RestError {
    match error {
        RetrievalError::EmptyScope => RestError::empty_scope(request_id),
        _ => RestError::internal(request_id),
    }
}

fn degradation_code(degradation: Degradation) -> &'static str {
    match degradation {
        Degradation::VectorUnavailable => "vector_unavailable",
        Degradation::LexicalUnavailable => "lexical_unavailable",
    }
}

impl From<GroundedHit> for SearchHitResponse {
    fn from(hit: GroundedHit) -> Self {
        Self {
            chunk_id: hit.chunk_id,
            document_id: hit.document_id,
            version_id: hit.version_id,
            collection_id: hit.collection_id,
            version_number: hit.version_number,
            snippet: hit.snippet,
            heading_path: if hit.heading_path.is_empty() {
                None
            } else {
                Some(hit.heading_path)
            },
            lexical_score: hit.lexical_score,
            vector_score: hit.vector_score,
            rerank_score: hit.rerank_score,
            page: hit.page,
            slide: hit.slide,
            sheet: hit.sheet,
            is_current: hit.is_current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_filter_is_intersection_only() {
        let allowed = Uuid::new_v4();
        let denied = Uuid::new_v4();
        let ctx =
            OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["qa.query"], [allowed]).unwrap();

        let narrowed = narrowed_context(&ctx, Some(vec![allowed, denied]));

        assert!(narrowed.allows_collection(allowed));
        assert!(!narrowed.allows_collection(denied));
    }

    #[test]
    fn query_length_is_bounded() {
        let ctx = OrgContext::try_new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ["qa.query"],
            [Uuid::new_v4()],
        )
        .unwrap();
        let body = SearchRequestBody {
            query: "x".repeat(MAX_QUERY_CHARS + 1),
            limit: None,
            collection_ids: None,
        };

        assert!(validate_search_request(body, &ctx, "req").is_err());
    }

    #[test]
    fn collection_ids_length_is_bounded() {
        let collection = Uuid::new_v4();
        let ctx = OrgContext::try_new(Uuid::new_v4(), Uuid::new_v4(), ["qa.query"], [collection])
            .unwrap();
        let body = SearchRequestBody {
            query: "hello".into(),
            limit: None,
            collection_ids: Some(vec![collection; MAX_COLLECTION_IDS + 1]),
        };

        assert!(validate_search_request(body, &ctx, "req").is_err());
    }

    #[test]
    fn search_response_fixture_matches_wire_dto() {
        let response = SearchResponse {
            hits: vec![SearchHitResponse {
                chunk_id: Uuid::parse_str("c3010000-0000-4000-8000-000000000001").unwrap(),
                document_id: Uuid::parse_str("d4010000-0000-4000-8000-000000000001").unwrap(),
                version_id: Uuid::parse_str("e5010000-0000-4000-8000-000000000001").unwrap(),
                collection_id: Uuid::parse_str("f6010000-0000-4000-8000-000000000001").unwrap(),
                version_number: 3,
                snippet: "Payment is due within 30 days.".into(),
                heading_path: Some(vec!["Contract".into(), "Payment".into()]),
                lexical_score: 0.75,
                vector_score: 0.5,
                rerank_score: 0.25,
                page: Some(4),
                slide: None,
                sheet: None,
                is_current: true,
            }],
            degraded: None,
        };
        let expected: serde_json::Value =
            serde_json::from_str(include_str!("../../openapi/fixtures/search_response.json"))
                .unwrap();

        assert_eq!(serde_json::to_value(response).unwrap(), expected);
    }
}
