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
//! `similarity` edges are opt-in on Qdrant + an embedding runtime being
//! configured (`AppState::vector_index()`/`AppState::embedder()`, same gate
//! `services/retrieval` uses) and are not implemented in this pass — see the
//! P2-17 report/backlog entry for why (no Qdrant instance in this sandbox to
//! validate against). When neither is configured, or once implemented,
//! finds nothing, the endpoint still returns `conflict`/`co_citation` edges
//! — it never fails just because vector search is unavailable.

use std::collections::BTreeMap;
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
use crate::db::graph::{self, GraphDocumentRow};
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::services::graph::{connected_components, prune, saturating_weight, GraphEdgeInput};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/graph", get(get_graph))
}

/// Contract: "Bounded: cap nodes/edges (vd 500/2000)".
const MAX_NODES: usize = 500;
const MAX_EDGES: usize = 2000;
/// Safety bound on the raw document fetch, generous relative to
/// `MAX_NODES` so degree-based `prune()` — not query truncation — decides
/// which 500 nodes survive when a tenant has more visible documents than
/// the cap.
const FETCH_SAFETY_CAP: i64 = 4000;

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

    let (documents, conflict_pairs, co_citation_pairs) =
        with_org_txn(state.pool(), &auth.context, {
            let ctx = auth.context.clone();
            let collection_filter = query.collection_id;
            move |txn| {
                Box::pin(async move {
                    let documents = graph::list_visible_documents(
                        txn,
                        &ctx,
                        collection_filter,
                        FETCH_SAFETY_CAP,
                    )
                    .await?;
                    let node_ids: Vec<Uuid> = documents.iter().map(|d| d.id).collect();
                    let conflict_pairs = graph::conflict_edges_among(txn, &ctx, &node_ids).await?;
                    let co_citation_pairs =
                        graph::co_citation_edges_among(txn, &ctx, &node_ids).await?;
                    Ok((documents, conflict_pairs, co_citation_pairs))
                })
            }
        })
        .await
        .map_err(|error| RouteError::from_db(error, &auth.request_id))?;

    let node_ids: Vec<Uuid> = documents.iter().map(|d| d.id).collect();

    let mut edges: Vec<GraphEdgeInput> = Vec::new();
    for (a, b, count) in &conflict_pairs {
        edges.push(GraphEdgeInput {
            source: *a,
            target: *b,
            kind: "conflict".to_string(),
            weight: saturating_weight(*count),
        });
    }
    for (a, b, count) in &co_citation_pairs {
        edges.push(GraphEdgeInput {
            source: *a,
            target: *b,
            kind: "co_citation".to_string(),
            weight: saturating_weight(*count),
        });
    }
    // similarity: see module doc — opt-in on Qdrant + embedder, not
    // implemented in this pass. `state.vector_index()`/`state.embedder()` are
    // the real availability check a future pass would gate on.
    let _similarity_available = state.vector_index().is_some() && state.embedder().is_some();

    let pruned = prune(&node_ids, &edges, MAX_NODES, MAX_EDGES);

    let mut degree: BTreeMap<Uuid, i64> = pruned.node_ids.iter().map(|id| (*id, 0)).collect();
    for edge in &pruned.edges {
        *degree.entry(edge.source).or_insert(0) += 1;
        *degree.entry(edge.target).or_insert(0) += 1;
    }

    let doc_by_id: BTreeMap<Uuid, &GraphDocumentRow> =
        documents.iter().map(|d| (d.id, d)).collect();

    let nodes: Vec<GraphNodeDto> = pruned
        .node_ids
        .iter()
        .filter_map(|id| {
            doc_by_id.get(id).map(|d| GraphNodeDto {
                id: d.id,
                title: d.title.clone(),
                collection_id: d.collection_id,
                collection_name: d.collection_name.clone(),
                status: d.state.clone(),
                degree: *degree.get(id).unwrap_or(&0),
            })
        })
        .collect();

    let edges_dto: Vec<GraphEdgeDto> = pruned
        .edges
        .iter()
        .map(|edge| GraphEdgeDto {
            source: edge.source,
            target: edge.target,
            kind: edge.kind.clone(),
            weight: edge.weight,
        })
        .collect();

    let components = connected_components(&pruned.node_ids, &pruned.edges);
    let communities: Vec<GraphCommunityDto> = components
        .iter()
        .enumerate()
        .map(|(index, component)| {
            let label = community_label(&component.node_ids, &doc_by_id);
            GraphCommunityDto {
                id: format!("community-{index}"),
                label,
                node_ids: component.node_ids.clone(),
                size: component.node_ids.len() as i64,
            }
        })
        .collect();

    Ok(Json(GraphResponseDto {
        nodes,
        edges: edges_dto,
        communities,
        request_id: auth.request_id,
    }))
}

/// Contract: community label is "tên bộ sưu tập chi phối hoặc title tài liệu
/// bậc cao nhất trong cụm" — a lone-document component labels itself by that
/// document's title; a multi-document component labels itself by whichever
/// collection has the most members in it (ties broken lexicographically so
/// the choice is deterministic, not by insertion order).
fn community_label(node_ids: &[Uuid], doc_by_id: &BTreeMap<Uuid, &GraphDocumentRow>) -> String {
    if let [only] = node_ids {
        return doc_by_id
            .get(only)
            .map(|d| d.title.clone())
            .unwrap_or_else(|| "Không xác định".to_string());
    }
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for id in node_ids {
        if let Some(document) = doc_by_id.get(id) {
            *counts.entry(document.collection_name.as_str()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| "Không xác định".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn doc(id: Uuid, title: &str, collection_name: &str) -> GraphDocumentRow {
        GraphDocumentRow {
            id,
            title: title.to_string(),
            collection_id: Uuid::new_v4(),
            collection_name: collection_name.to_string(),
            state: "indexed".to_string(),
        }
    }

    #[test]
    fn singleton_community_labels_by_document_title() {
        let a = doc(uid(1), "Chính sách nghỉ phép", "Nhân sự");
        let doc_by_id: BTreeMap<Uuid, &GraphDocumentRow> = [(a.id, &a)].into_iter().collect();
        assert_eq!(
            community_label(&[uid(1)], &doc_by_id),
            "Chính sách nghỉ phép"
        );
    }

    #[test]
    fn multi_node_community_labels_by_majority_collection_name() {
        let a = doc(uid(1), "A", "Nhân sự");
        let b = doc(uid(2), "B", "Nhân sự");
        let c = doc(uid(3), "C", "Tài chính");
        let doc_by_id: BTreeMap<Uuid, &GraphDocumentRow> =
            [(a.id, &a), (b.id, &b), (c.id, &c)].into_iter().collect();
        assert_eq!(
            community_label(&[uid(1), uid(2), uid(3)], &doc_by_id),
            "Nhân sự"
        );
    }

    #[test]
    fn multi_node_community_tie_breaks_lexicographically() {
        let a = doc(uid(1), "A", "Zeta");
        let b = doc(uid(2), "B", "Alpha");
        let doc_by_id: BTreeMap<Uuid, &GraphDocumentRow> =
            [(a.id, &a), (b.id, &b)].into_iter().collect();
        assert_eq!(community_label(&[uid(1), uid(2)], &doc_by_id), "Alpha");
    }
}
