//! Search API routes backed by the tenant-scoped retrieval engine.

use std::sync::Arc;

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
use crate::services::retrieval::{
    retrieve, Degradation, GroundedHit, RetrievalError, RetrievalRequest, VersionMode,
    MAX_RETRIEVAL_LIMIT,
};

pub(crate) const JSON_BODY_LIMIT: usize = 16 * 1024;
pub(crate) const MAX_QUERY_CHARS: usize = 4096;

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
    require_permission_or_403(&auth.context, "qa.query", &request_id)?;
    let input = validate_search_request(body, &auth.context, &request_id)?;
    let response = retrieve(
        state.pool(),
        state.qdrant(),
        &input.ctx,
        RetrievalRequest {
            query: input.query,
            limit: input.limit,
            mode: VersionMode::Current,
        },
    )
    .await
    .map_err(|error| map_retrieval_error(error, &request_id))?;

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
    let limit = validate_limit(body.limit, MAX_RETRIEVAL_LIMIT, request_id)?;
    Ok(ValidSearchRequest {
        query,
        limit,
        ctx: narrowed_context(ctx, body.collection_ids),
    })
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
}
