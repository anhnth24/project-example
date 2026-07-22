//! `POST /api/v1/search` — tenant-scoped hybrid retrieval (P1B-R05).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::{ApiRejection, AppJson};
use crate::auth::middleware::AuthenticatedOrg;
use crate::http::AppState;
use crate::routes::qa_common::{
    fresh_org_context, hit_to_json, parse_collection_ids, parse_query_text, parse_search_limit,
    parse_version_mode, require_history_if_needed, require_query_perm, run_hybrid_search,
    VersionModeBody,
};
use crate::services::retrieval::RetrievalRequest;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/search", post(search))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchBody {
    query: String,
    #[serde(default)]
    collection_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    mode: Option<VersionModeBody>,
    #[serde(default)]
    limit: Option<u32>,
}

async fn search(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppJson(body): AppJson<SearchBody>,
) -> Result<Json<serde_json::Value>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let query = parse_query_text(&body.query, "query", &request_id)?;
    let collection_ids = parse_collection_ids(body.collection_ids, &request_id)?;
    let mode = parse_version_mode(body.mode.as_ref(), &request_id)?;
    let limit = parse_search_limit(body.limit, &request_id)?;

    // Fresh OrgContext — JWT org/user are hints only.
    let ctx = fresh_org_context(
        &state,
        auth.context.org_id(),
        auth.context.user_id(),
        &request_id,
    )
    .await?;
    require_query_perm(&ctx, &request_id)?;
    require_history_if_needed(&ctx, &mode, &request_id)?;

    let retrieval = run_hybrid_search(
        &state,
        &ctx,
        RetrievalRequest {
            query,
            collection_ids,
            mode,
            limit,
            conflict_ids: vec![],
        },
        &request_id,
    )
    .await?;

    Ok(Json(json!({
        "hits": retrieval.hits.iter().map(hit_to_json).collect::<Vec<_>>(),
        "warnings": retrieval.warnings,
        "embeddingMode": retrieval.embedding_mode,
        "requestId": request_id
    })))
}
