//! Document Graph MVP (P2-17, owner request 2026-07-29 — see
//! `plans/markhand-web/backlog/phase-2/issues/README.md`): read-only graph
//! data for the new "Đồ thị" web page — nodes (caller-visible documents),
//! edges (`conflict` / `co_citation` / `similarity`), communities
//! (connected components of whatever survives pruning).
//!
//! Permission choice (contract asked to pick from real code, not invent
//! one): gated behind `qa.query`, the same permission
//! `routes/documents.rs::list_conflicts`/`get_conflict` already require even
//! though *listing documents themselves* (`list_documents`) needs no
//! permission beyond collection ACL. This endpoint always blends
//! conflict-derived and QA-history-derived (`co_citation`) edges into the
//! response — the sensitive part — so the whole endpoint is gated at that
//! existing precedent rather than a new `graph.read` permission with no
//! seeded role grants anywhere in the migrations yet.
//!
//! ACL: node visibility is the same allow-list every other list route uses
//! (`ctx.allowed_collection_ids()` / `ctx.allows_collection(...)`, see
//! `routes/collections.rs`/`routes/documents.rs`); edges are only ever built
//! from pairs already inside that visible node set (`db::graph`'s queries
//! take `node_ids` and additionally scope by `org_id`), so an edge can never
//! reference a document the caller could not otherwise see.
//!
//! `similarity` edges are opt-in on the vector index + an embedding runtime
//! being configured (`AppState::vector_index()`/`AppState::embedder()`, same
//! gate `services/retrieval` uses) — computed by
//! `services::graph::compute_similarity_edges` via the vector index's
//! recommend-by-id API (P2-17 follow-up; see that function's doc for the
//! pipeline). This route stays DTO-mapping only: it just checks both are
//! configured and passes a `services::graph::SimilarityDeps` through, never
//! reading the vector store itself. When neither is configured, or the
//! vector store errors, the endpoint still returns `conflict`/`co_citation`
//! edges — it never fails just because vector search is unavailable (see
//! `build_org_graph`'s doc).

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::{ApiError, GraphCommunityDto, GraphEdgeDto, GraphNodeDto, GraphResponseDto};
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::error::DbError;
use crate::http::AppState;
use crate::services::graph::{build_org_graph, SimilarityDeps};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/graph", get(get_graph))
}

#[derive(Debug, Deserialize)]
struct GraphQuery {
    #[serde(rename = "collectionId")]
    collection_id: Option<Uuid>,
}

async fn get_graph(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Query(query): Query<GraphQuery>,
) -> Result<Json<GraphResponseDto>, RouteError> {
    require_permission(&auth.context, "qa.query")
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    if let Some(collection_id) = query.collection_id {
        if !auth.context.allows_collection(collection_id) {
            return Err(RouteError::NotFound(auth.request_id.clone()));
        }
    }

    // similarity: see module doc — opt-in on both the vector index and an
    // embedder being configured; `None` when either is missing, which keeps
    // `build_org_graph`'s response identical to before this pass.
    let similarity = match (state.vector_index(), state.embedder()) {
        (Some(vector_index), Some(embedder)) => Some(SimilarityDeps {
            vector_index,
            embedder,
        }),
        _ => None,
    };

    let data = build_org_graph(state.pool(), &auth.context, query.collection_id, similarity)
        .await
        .map_err(|error| RouteError::from_db(error, &auth.request_id))?;

    let nodes: Vec<GraphNodeDto> = data
        .nodes
        .iter()
        .map(|n| GraphNodeDto {
            id: n.id,
            title: n.title.clone(),
            collection_id: n.collection_id,
            collection_name: n.collection_name.clone(),
            status: n.status.clone(),
            degree: n.degree,
        })
        .collect();

    let edges_dto: Vec<GraphEdgeDto> = data
        .edges
        .iter()
        .map(|edge| GraphEdgeDto {
            source: edge.source,
            target: edge.target,
            kind: edge.kind.clone(),
            weight: edge.weight,
        })
        .collect();

    let communities: Vec<GraphCommunityDto> = data
        .communities
        .iter()
        .enumerate()
        .map(|(index, community)| GraphCommunityDto {
            id: format!("community-{index}"),
            label: community.label.clone(),
            node_ids: community.node_ids.clone(),
            size: community.node_ids.len() as i64,
        })
        .collect();

    Ok(Json(GraphResponseDto {
        nodes,
        edges: edges_dto,
        communities,
        request_id: auth.request_id,
    }))
}

enum RouteError {
    Denied(String),
    NotFound(String),
    Database(String),
}

impl RouteError {
    fn from_db(error: DbError, request_id: &str) -> Self {
        match error {
            DbError::NotFound => Self::NotFound(request_id.to_string()),
            DbError::Config(message) if message == "collection_denied" => {
                Self::NotFound(request_id.to_string())
            }
            _ => Self::Database(request_id.to_string()),
        }
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        let (status, code, message, request_id) = match self {
            Self::Denied(request_id) => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "Permission denied",
                request_id,
            ),
            Self::NotFound(request_id) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Collection not found",
                request_id,
            ),
            Self::Database(request_id) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Request failed",
                request_id,
            ),
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
