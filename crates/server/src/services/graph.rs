//! Document Graph MVP (P2-17): pure algorithms (degree-based bounding/pruning,
//! connected components — DB-free, deterministic, unit-tested below) plus the
//! `build_org_graph` orchestration that fetches caller-visible nodes/edges
//! through `db::graph` (routes stay DTO-mapping only, per
//! check-architecture-boundaries).
//!
//! Deliberately hand-rolled instead of a graph crate: components/pruning are
//! small, well-understood algorithms (BFS + two sorts) and the project's own
//! dependency policy (`scripts/check-dependency-policy.py`) favors not
//! reaching for a dependency when ~100 lines of plain Rust cover the need —
//! same reasoning the web side applies to the force-directed layout.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::graph::{self as db_graph, GraphDocumentRow};
use crate::db::index_metadata;
use deadpool_postgres::Pool;

use crate::db::pool::with_org_txn;
use crate::services::embedding::ApprovedEmbeddingRuntime;
use crate::services::index_signature::{collection_name_for_digest, CollectionName};
use crate::services::retrieval::generation_compatible_with_runtime;
use crate::storage::error::StorageError;
use crate::storage::qdrant::{QdrantClient, VectorScope};

/// One candidate edge before pruning. `kind` is carried through so the same
/// document pair can carry more than one edge (e.g. both a `conflict` and a
/// `co_citation` edge) — pruning/components treat those as distinct edges.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdgeInput {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: String,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PrunedGraph {
    /// Kept node ids, ascending — deterministic regardless of input order.
    pub node_ids: Vec<Uuid>,
    /// Kept edges, ranked by weight desc (ties by source/target/kind asc)
    /// before truncation, but returned in that same ranked order.
    pub edges: Vec<GraphEdgeInput>,
}

/// Degree-based bound matching the contract's "cap nodes/edges (vd
/// 500/2000), degree-based pruning khi vượt":
///
/// 1. Rank nodes by degree (edge endpoint count) descending, ties broken by
///    ascending id, and keep at most `max_nodes`. This favors well-connected
///    ("central") documents over arbitrarily-ordered ones when a tenant has
///    more visible documents than the cap allows.
/// 2. Drop any edge with an endpoint that didn't survive step 1.
/// 3. Rank the remaining edges by weight descending, ties broken by
///    `(source, target, kind)` ascending, and keep at most `max_edges`.
pub fn prune(
    node_ids: &[Uuid],
    edges: &[GraphEdgeInput],
    max_nodes: usize,
    max_edges: usize,
) -> PrunedGraph {
    let mut degree: BTreeMap<Uuid, usize> = node_ids.iter().map(|id| (*id, 0)).collect();
    for edge in edges {
        if let Some(d) = degree.get_mut(&edge.source) {
            *d += 1;
        }
        if let Some(d) = degree.get_mut(&edge.target) {
            *d += 1;
        }
    }

    let mut ranked_nodes: Vec<Uuid> = node_ids.to_vec();
    ranked_nodes.sort_by(|a, b| {
        let da = degree.get(a).copied().unwrap_or(0);
        let db = degree.get(b).copied().unwrap_or(0);
        db.cmp(&da).then_with(|| a.cmp(b))
    });
    let kept: BTreeSet<Uuid> = ranked_nodes.into_iter().take(max_nodes).collect();

    let mut kept_edges: Vec<GraphEdgeInput> = edges
        .iter()
        .filter(|edge| kept.contains(&edge.source) && kept.contains(&edge.target))
        .cloned()
        .collect();
    kept_edges.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.source, a.target, &a.kind).cmp(&(b.source, b.target, &b.kind)))
    });
    kept_edges.truncate(max_edges);

    let mut node_ids_out: Vec<Uuid> = kept.into_iter().collect();
    node_ids_out.sort();
    PrunedGraph {
        node_ids: node_ids_out,
        edges: kept_edges,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    /// Members, ascending — deterministic regardless of input order.
    pub node_ids: Vec<Uuid>,
}

/// Connected components over an undirected graph built from `edges`'
/// endpoints (edge `kind`/`weight` do not affect connectivity — a
/// `conflict` edge and a `co_citation` edge between the same pair merge
/// them into the same component either way). Isolated nodes form their own
/// singleton component.
///
/// Deterministic: components are ordered by their smallest member id, and
/// each component's members are sorted ascending — independent of the order
/// `node_ids`/`edges` are given in.
pub fn connected_components(node_ids: &[Uuid], edges: &[GraphEdgeInput]) -> Vec<Component> {
    let mut adjacency: BTreeMap<Uuid, BTreeSet<Uuid>> =
        node_ids.iter().map(|id| (*id, BTreeSet::new())).collect();
    for edge in edges {
        adjacency
            .entry(edge.source)
            .or_default()
            .insert(edge.target);
        adjacency
            .entry(edge.target)
            .or_default()
            .insert(edge.source);
    }

    let mut sorted_nodes = node_ids.to_vec();
    sorted_nodes.sort();
    sorted_nodes.dedup();

    let mut visited: BTreeSet<Uuid> = BTreeSet::new();
    let mut components = Vec::new();
    for &start in &sorted_nodes {
        if visited.contains(&start) {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        visited.insert(start);
        let mut members = Vec::new();
        while let Some(current) = queue.pop_front() {
            members.push(current);
            if let Some(neighbors) = adjacency.get(&current) {
                for &next in neighbors {
                    if visited.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
        members.sort();
        components.push(Component { node_ids: members });
    }
    components.sort_by_key(|c| c.node_ids.first().copied());
    components
}

/// Maps an evidence count to a `0..1` edge weight: saturating, not linear,
/// so a single conflict/co-citation still registers a real (if modest)
/// weight and repeated evidence pushes it toward — but never reaches — 1.0.
/// Deliberately not a claim of calibrated probability, just a bounded,
/// monotone strength signal for layout/rendering.
pub fn saturating_weight(count: i64) -> f64 {
    1.0 - 1.0 / (1.0 + count.max(1) as f64)
}

// ---------------------------------------------------------------------
// Orchestration (route -> service -> db, per check-architecture-boundaries):
// fetch the caller-visible nodes and real-relation edges, then bound and
// cluster them. Routes stay DTO-mapping only.
// ---------------------------------------------------------------------

/// Contract: "Bounded: cap nodes/edges (vd 500/2000)".
pub const MAX_NODES: usize = 500;
pub const MAX_EDGES: usize = 2000;
/// Safety bound on the raw document fetch, generous relative to
/// `MAX_NODES` so degree-based `prune()` — not query truncation — decides
/// which 500 nodes survive when a tenant has more visible documents than
/// the cap.
const FETCH_SAFETY_CAP: i64 = 4000;

// ---------------------------------------------------------------------
// `similarity` edges (P2-17 follow-up): opt-in on a configured vector index
// + embedder, computed via Qdrant's recommend-by-id API — never by scrolling
// whole vectors back into the app. See `compute_similarity_edges` below for
// the full pipeline; the constants here are its tunable knobs.
// ---------------------------------------------------------------------

/// Cross-document neighbors requested per document from Qdrant's recommend API.
const SIMILARITY_TOP_K: usize = 5;
/// Minimum Qdrant Cosine score (`[-1, 1]`) to keep a similarity edge at all.
/// A fresh constant for the graph — not reused from retrieval's hybrid
/// rerank score, which blends lexical + vector signals and lives on a
/// different scale entirely.
pub const SIMILARITY_SCORE_THRESHOLD: f32 = 0.5;
/// How many of a document's own current chunks are sent as Qdrant `positive`
/// ids. Small and fixed so the recommend request body stays cheap regardless
/// of document size.
const SIMILARITY_POSITIVE_CHUNKS_PER_DOC: usize = 3;
/// Page size used while scrolling for representative chunk point ids.
const SIMILARITY_SCROLL_PAGE_LIMIT: usize = 512;
/// Bounds how many scroll pages we fetch while looking for representative
/// chunk ids across the whole node set — caps total scanned points at
/// `SIMILARITY_SCROLL_PAGE_LIMIT * SIMILARITY_SCROLL_MAX_PAGES` regardless of
/// how many chunks any single document has.
const SIMILARITY_SCROLL_MAX_PAGES: usize = 8;
/// Bounds the number of Qdrant recommend round-trips this endpoint makes —
/// one per document that has at least one representative chunk. Documents
/// beyond this cap simply get no similarity edges (conflict/co_citation
/// edges are unaffected, and which documents are affected is deterministic:
/// `list_visible_documents`' `ORDER BY created_at DESC, id DESC` decides
/// iteration order). A single batched call (Qdrant's `points/recommend/batch`)
/// would remove this cap entirely — left for a follow-up, see the P2-17
/// report.
const SIMILARITY_NODE_CAP: usize = 200;
/// Cap applied to the raw similarity edge set before it is merged with
/// conflict/co_citation edges and hits the shared `prune()` — keeps one
/// signal from crowding out the others ahead of the overall `MAX_EDGES`.
const MAX_SIMILARITY_EDGES: usize = 500;

/// Similarity dependencies, injected by the route from
/// `AppState::vector_index()` / `AppState::embedder()` — `services::graph`
/// deliberately does not read `AppState` itself (no precedent for services
/// doing so; see how `services::retrieval::hybrid_search` instead takes
/// `&QdrantClient` + `Option<&ApprovedEmbeddingRuntime>` as plain
/// parameters). Keeping the service AppState-free is also what lets
/// `tests/graph.rs`'s Qdrant-gated integration test call `build_org_graph`
/// directly against a real `QdrantClient` without booting the whole app.
#[derive(Clone, Copy)]
pub struct SimilarityDeps<'a> {
    pub vector_index: &'a QdrantClient,
    pub embedder: &'a ApprovedEmbeddingRuntime,
}

#[derive(Debug, Clone)]
pub struct GraphNodeData {
    pub id: Uuid,
    pub title: String,
    pub collection_id: Uuid,
    pub collection_name: String,
    pub status: String,
    pub degree: i64,
}

#[derive(Debug, Clone)]
pub struct GraphCommunityData {
    pub label: String,
    pub node_ids: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct GraphData {
    pub nodes: Vec<GraphNodeData>,
    pub edges: Vec<GraphEdgeInput>,
    pub communities: Vec<GraphCommunityData>,
}

/// Builds the bounded, clustered document graph the route serves: visible
/// documents (RLS + caller ACL, exactly the library's visibility) plus
/// `conflict`, `co_citation`, and (when `similarity` is configured)
/// `similarity` edges among them.
///
/// `similarity` is `None` when the vector index and/or embedder is not
/// configured — the response is unaffected (same as before this pass). When
/// it is configured, a Qdrant error during similarity computation is
/// swallowed (no edges added) rather than failing the whole graph — the
/// endpoint must keep returning `conflict`/`co_citation` edges even if the
/// vector store is unavailable.
pub async fn build_org_graph(
    pool: &Pool,
    ctx: &OrgContext,
    collection_filter: Option<Uuid>,
    similarity: Option<SimilarityDeps<'_>>,
) -> Result<GraphData, DbError> {
    let (documents, conflict_pairs, co_citation_pairs) = with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let documents = db_graph::list_visible_documents(
                    txn,
                    &ctx,
                    collection_filter,
                    FETCH_SAFETY_CAP,
                )
                .await?;
                let node_ids: Vec<Uuid> = documents.iter().map(|d| d.id).collect();
                let conflict_pairs = db_graph::conflict_edges_among(txn, &ctx, &node_ids).await?;
                let co_citation_pairs =
                    db_graph::co_citation_edges_among(txn, &ctx, &node_ids).await?;
                Ok((documents, conflict_pairs, co_citation_pairs))
            })
        }
    })
    .await?;

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

    if let Some(deps) = similarity {
        match compute_similarity_edges(pool, ctx, deps, &documents).await {
            Ok(similarity_edges) => edges.extend(similarity_edges),
            Err(_) => {
                // Fail-soft: see this function's doc comment — a vector
                // store hiccup must not fail the whole graph.
            }
        }
    }

    let pruned = prune(&node_ids, &edges, MAX_NODES, MAX_EDGES);

    let mut degree: BTreeMap<Uuid, i64> = pruned.node_ids.iter().map(|id| (*id, 0)).collect();
    for edge in &pruned.edges {
        *degree.entry(edge.source).or_insert(0) += 1;
        *degree.entry(edge.target).or_insert(0) += 1;
    }

    let doc_by_id: BTreeMap<Uuid, &GraphDocumentRow> =
        documents.iter().map(|d| (d.id, d)).collect();

    let nodes: Vec<GraphNodeData> = pruned
        .node_ids
        .iter()
        .filter_map(|id| {
            doc_by_id.get(id).map(|d| GraphNodeData {
                id: d.id,
                title: d.title.clone(),
                collection_id: d.collection_id,
                collection_name: d.collection_name.clone(),
                status: d.state.clone(),
                degree: *degree.get(id).unwrap_or(&0),
            })
        })
        .collect();

    let communities: Vec<GraphCommunityData> =
        connected_components(&pruned.node_ids, &pruned.edges)
            .iter()
            .map(|component| GraphCommunityData {
                label: community_label(&component.node_ids, &doc_by_id),
                node_ids: component.node_ids.clone(),
            })
            .collect();

    Ok(GraphData {
        nodes,
        edges: pruned.edges,
        communities,
    })
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

// ---------------------------------------------------------------------
// `similarity` edges: Qdrant recommend-by-id, aggregate, threshold, cap.
// ---------------------------------------------------------------------

/// Computes `similarity` edges among `documents` (the same visible node set
/// `conflict`/`co_citation` edges are built from). For each document with at
/// least one representative current chunk point, asks Qdrant to recommend
/// its top-`SIMILARITY_TOP_K` cross-document neighbors from vectors already
/// indexed by `services::indexing` — no vector ever leaves Qdrant into this
/// process. Only generations whose signature matches `deps.embedder`'s plan
/// are queried (mirrors `services::retrieval::generation_compatible_with_runtime`,
/// so this never sends a query into a Qdrant collection built for a
/// different embedding family/dimensionality).
async fn compute_similarity_edges(
    pool: &Pool,
    ctx: &OrgContext,
    deps: SimilarityDeps<'_>,
    documents: &[GraphDocumentRow],
) -> Result<Vec<GraphEdgeInput>, StorageError> {
    if documents.is_empty() {
        return Ok(Vec::new());
    }
    let plan = deps.embedder.plan();
    let Some(query_dimensions) = plan.expected_dimensions() else {
        return Ok(Vec::new());
    };

    let doc_by_id: BTreeMap<Uuid, &GraphDocumentRow> =
        documents.iter().map(|d| (d.id, d)).collect();
    let collection_ids: Vec<Uuid> = documents.iter().map(|d| d.collection_id).collect();
    let active = with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                index_metadata::list_active_for_collections(txn, &ctx, &collection_ids).await
            })
        }
    })
    .await
    .map_err(|_| StorageError::Backend)?;

    let mut by_signature: BTreeMap<String, BTreeSet<Uuid>> = BTreeMap::new();
    for meta in active {
        if !generation_compatible_with_runtime(&meta, plan, query_dimensions) {
            continue;
        }
        if let Some(collection_id) = meta.collection_id {
            by_signature
                .entry(meta.index_signature_sha256)
                .or_default()
                .insert(collection_id);
        }
    }
    if by_signature.is_empty() {
        return Ok(Vec::new());
    }

    let mut raw_pairs: Vec<(Uuid, Uuid, f32)> = Vec::new();
    let mut processed = 0usize;

    'groups: for (digest, group_collections) in by_signature {
        if processed >= SIMILARITY_NODE_CAP {
            break;
        }
        let collection_name = collection_name_for_digest(&digest)?;
        let scope = VectorScope::new(ctx.org_id(), group_collections.iter().copied());

        let target_docs: BTreeSet<Uuid> = documents
            .iter()
            .filter(|d| group_collections.contains(&d.collection_id))
            .map(|d| d.id)
            .collect();
        if target_docs.is_empty() {
            continue;
        }

        let representatives =
            representative_chunk_points(deps.vector_index, &collection_name, &scope, &target_docs)
                .await?;
        let current_only = [json!({ "key": "is_current", "match": { "value": true } })];

        for (document_id, positive_ids) in &representatives {
            if positive_ids.is_empty() {
                continue;
            }
            if processed >= SIMILARITY_NODE_CAP {
                break 'groups;
            }
            processed += 1;

            let hits = deps
                .vector_index
                .recommend(
                    &collection_name,
                    &scope,
                    positive_ids,
                    &current_only,
                    SIMILARITY_TOP_K,
                )
                .await?;
            for hit in hits {
                // A document's other chunks are not a cross-document
                // neighbor; a neighbor outside the caller-visible node set
                // must never surface as an edge (same invariant
                // `routes/graph.rs` documents for conflict/co_citation).
                if hit.payload.document_id == *document_id
                    || !doc_by_id.contains_key(&hit.payload.document_id)
                {
                    continue;
                }
                raw_pairs.push((*document_id, hit.payload.document_id, hit.score));
            }
        }
    }

    let edges = aggregate_similarity_pairs(raw_pairs, SIMILARITY_SCORE_THRESHOLD)
        .into_iter()
        .map(|(a, b, score)| GraphEdgeInput {
            source: a,
            target: b,
            kind: "similarity".to_string(),
            weight: normalize_similarity_weight(score),
        })
        .collect();
    Ok(cap_similarity_edges(edges, MAX_SIMILARITY_EDGES))
}

/// Gathers up to `SIMILARITY_POSITIVE_CHUNKS_PER_DOC` current chunk point
/// ids per document in `target_document_ids`, by scrolling Qdrant (payload +
/// point id only — never vectors, see `QdrantClient::scroll_points_page`'s
/// `with_vector: false`). Bounded by `SIMILARITY_SCROLL_MAX_PAGES`: a
/// document with no current chunk indexed yet (or one this scan didn't
/// reach) simply gets no entry and is skipped by the caller, exactly like a
/// missing vector index is already handled elsewhere in this pipeline.
async fn representative_chunk_points(
    qdrant: &QdrantClient,
    collection_name: &CollectionName,
    scope: &VectorScope,
    target_document_ids: &BTreeSet<Uuid>,
) -> Result<BTreeMap<Uuid, Vec<Uuid>>, StorageError> {
    let mut representatives: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    let document_ids: Vec<Value> = target_document_ids
        .iter()
        .map(|id| Value::String(id.to_string()))
        .collect();
    let extra_must = [
        json!({ "key": "document_id", "match": { "any": document_ids } }),
        json!({ "key": "is_current", "match": { "value": true } }),
    ];

    let mut offset: Option<Value> = None;
    for _ in 0..SIMILARITY_SCROLL_MAX_PAGES {
        let page = qdrant
            .scroll_points_page(
                collection_name,
                scope,
                &extra_must,
                SIMILARITY_SCROLL_PAGE_LIMIT,
                offset.clone(),
            )
            .await?;
        for (point_id, payload) in page.points {
            let bucket = representatives.entry(payload.document_id).or_default();
            if bucket.len() < SIMILARITY_POSITIVE_CHUNKS_PER_DOC {
                bucket.push(point_id);
            }
        }
        match page.next_page_offset {
            Some(next) if representatives.len() < target_document_ids.len() => offset = Some(next),
            _ => break,
        }
    }
    Ok(representatives)
}

/// Canonicalizes each unordered document pair (`(min, max)` by id) and keeps
/// the max score seen for it, dropping anything below `threshold`. Max (not
/// mean) is deliberate: a single very close chunk pair between two documents
/// is a meaningful signal even when their other chunks are unrelated — mean
/// would dilute it toward the documents' average content, the same
/// "any evidence counts" reasoning `conflict`/`co_citation` edges already
/// use (see `db::graph::conflict_edges_among`/`co_citation_edges_among`).
pub fn aggregate_similarity_pairs(
    raw: Vec<(Uuid, Uuid, f32)>,
    threshold: f32,
) -> Vec<(Uuid, Uuid, f32)> {
    let mut best: BTreeMap<(Uuid, Uuid), f32> = BTreeMap::new();
    for (a, b, score) in raw {
        if a == b || score < threshold {
            continue;
        }
        let key = if a < b { (a, b) } else { (b, a) };
        best.entry(key)
            .and_modify(|existing| {
                if score > *existing {
                    *existing = score;
                }
            })
            .or_insert(score);
    }
    best.into_iter()
        .map(|((a, b), score)| (a, b, score))
        .collect()
}

/// Maps a Qdrant Cosine score (range `[-1, 1]`) onto the contract's `0..1`
/// edge weight. Approved embedding runtimes are expected to score mostly
/// non-negative for genuinely related chunks in practice, but the map covers
/// the full Cosine range so a pathological negative score still lands
/// in-bounds instead of silently clamping to an uninformative 0.
pub fn normalize_similarity_weight(score: f32) -> f64 {
    (((score as f64) + 1.0) / 2.0).clamp(0.0, 1.0)
}

/// Caps the similarity edge set before it is merged with conflict/co_citation
/// edges, ranked the same way `prune()` ranks edges (weight desc, ties by
/// `(source, target)` asc) so truncation is deterministic.
pub fn cap_similarity_edges(mut edges: Vec<GraphEdgeInput>, cap: usize) -> Vec<GraphEdgeInput> {
    edges.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (a.source, a.target).cmp(&(b.source, b.target)))
    });
    edges.truncate(cap);
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn edge(a: u128, b: u128, kind: &str, weight: f64) -> GraphEdgeInput {
        GraphEdgeInput {
            source: uid(a),
            target: uid(b),
            kind: kind.to_string(),
            weight,
        }
    }

    #[test]
    fn components_groups_connected_nodes_and_isolates_singletons() {
        let nodes = vec![uid(1), uid(2), uid(3), uid(4)];
        let edges = vec![edge(1, 2, "conflict", 0.5)];
        let components = connected_components(&nodes, &edges);
        assert_eq!(
            components,
            vec![
                Component {
                    node_ids: vec![uid(1), uid(2)]
                },
                Component {
                    node_ids: vec![uid(3)]
                },
                Component {
                    node_ids: vec![uid(4)]
                },
            ]
        );
    }

    #[test]
    fn components_are_order_independent() {
        let nodes_a = vec![uid(1), uid(2), uid(3)];
        let nodes_b = vec![uid(3), uid(1), uid(2)];
        let edges_a = vec![edge(2, 3, "co_citation", 0.2), edge(1, 2, "conflict", 0.8)];
        let edges_b = vec![edge(1, 2, "conflict", 0.8), edge(2, 3, "co_citation", 0.2)];
        assert_eq!(
            connected_components(&nodes_a, &edges_a),
            connected_components(&nodes_b, &edges_b)
        );
    }

    #[test]
    fn components_merge_multi_kind_edges_between_same_pair() {
        let nodes = vec![uid(1), uid(2)];
        let edges = vec![edge(1, 2, "conflict", 0.9), edge(1, 2, "co_citation", 0.3)];
        let components = connected_components(&nodes, &edges);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].node_ids, vec![uid(1), uid(2)]);
    }

    #[test]
    fn prune_keeps_highest_degree_nodes_when_over_cap() {
        // Hub (1) connects to 2,3,4; 5 and 6 are an isolated pair.
        let nodes = vec![uid(1), uid(2), uid(3), uid(4), uid(5), uid(6)];
        let edges = vec![
            edge(1, 2, "conflict", 0.5),
            edge(1, 3, "conflict", 0.5),
            edge(1, 4, "conflict", 0.5),
            edge(5, 6, "conflict", 0.5),
        ];
        let pruned = prune(&nodes, &edges, 4, 100);
        // Degrees: 1->3, 2->1, 3->1, 4->1, 5->1, 6->1. Node 1 always kept;
        // the tie among {2,3,4,5,6} (all degree 1) is broken by ascending id,
        // so 2,3,4 (the smallest three ids after 1) survive over 5,6.
        assert_eq!(pruned.node_ids, vec![uid(1), uid(2), uid(3), uid(4)]);
    }

    #[test]
    fn prune_drops_edges_with_a_dropped_endpoint() {
        let nodes = vec![uid(1), uid(2), uid(3)];
        let edges = vec![edge(1, 2, "conflict", 0.9), edge(2, 3, "conflict", 0.9)];
        let pruned = prune(&nodes, &edges, 2, 100);
        assert_eq!(pruned.node_ids, vec![uid(1), uid(2)]);
        assert_eq!(pruned.edges, vec![edge(1, 2, "conflict", 0.9)]);
    }

    #[test]
    fn prune_keeps_highest_weight_edges_when_over_cap() {
        let nodes = vec![uid(1), uid(2), uid(3), uid(4)];
        let edges = vec![
            edge(1, 2, "conflict", 0.1),
            edge(1, 3, "conflict", 0.9),
            edge(1, 4, "conflict", 0.5),
        ];
        let pruned = prune(&nodes, &edges, 10, 2);
        assert_eq!(
            pruned.edges,
            vec![edge(1, 3, "conflict", 0.9), edge(1, 4, "conflict", 0.5)]
        );
    }

    #[test]
    fn prune_is_deterministic_regardless_of_input_order() {
        let nodes_a = vec![uid(1), uid(2), uid(3)];
        let nodes_b = vec![uid(3), uid(2), uid(1)];
        let edges_a = vec![edge(1, 2, "conflict", 0.4), edge(2, 3, "conflict", 0.6)];
        let edges_b = vec![edge(2, 3, "conflict", 0.6), edge(1, 2, "conflict", 0.4)];
        assert_eq!(
            prune(&nodes_a, &edges_a, 10, 10),
            prune(&nodes_b, &edges_b, 10, 10)
        );
    }

    #[test]
    fn saturating_weight_is_bounded_monotone_and_never_reaches_one() {
        assert!(saturating_weight(1) > 0.0);
        assert!(saturating_weight(1) < saturating_weight(2));
        assert!(saturating_weight(2) < saturating_weight(100));
        assert!(saturating_weight(1_000_000) < 1.0);
        assert_eq!(saturating_weight(0), saturating_weight(1)); // count clamped to >= 1
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

    // -------------------------------------------------------------
    // `similarity` edges: pure aggregate/threshold/cap unit tests
    // (no Qdrant — the Qdrant-gated path is covered by the `#[ignore]`d
    // integration test in `tests/graph.rs`, run only with
    // `MARKHAND_TEST_QDRANT_URL`).
    // -------------------------------------------------------------

    #[test]
    fn aggregate_drops_scores_below_threshold() {
        let raw = vec![(uid(1), uid(2), 0.49), (uid(3), uid(4), 0.5)];
        let kept = aggregate_similarity_pairs(raw, 0.5);
        assert_eq!(kept, vec![(uid(3), uid(4), 0.5)]);
    }

    #[test]
    fn aggregate_canonicalizes_pair_order_regardless_of_direction() {
        // Same pair discovered from both directions (doc 2's recommend call
        // found doc 1, and vice versa) must collapse into one edge.
        let raw = vec![(uid(2), uid(1), 0.6), (uid(1), uid(2), 0.7)];
        let kept = aggregate_similarity_pairs(raw, 0.0);
        assert_eq!(kept, vec![(uid(1), uid(2), 0.7)]);
    }

    #[test]
    fn aggregate_keeps_max_not_mean_across_duplicate_chunk_pairs() {
        let raw = vec![
            (uid(1), uid(2), 0.55),
            (uid(1), uid(2), 0.9),
            (uid(1), uid(2), 0.6),
        ];
        let kept = aggregate_similarity_pairs(raw, 0.0);
        assert_eq!(kept, vec![(uid(1), uid(2), 0.9)]);
    }

    #[test]
    fn aggregate_drops_self_pairs() {
        let raw = vec![(uid(1), uid(1), 0.99)];
        assert!(aggregate_similarity_pairs(raw, 0.0).is_empty());
    }

    #[test]
    fn normalize_weight_maps_cosine_range_into_zero_one() {
        assert_eq!(normalize_similarity_weight(1.0), 1.0);
        assert_eq!(normalize_similarity_weight(-1.0), 0.0);
        assert_eq!(normalize_similarity_weight(0.0), 0.5);
        // Pathological out-of-range score still clamps in-bounds.
        assert_eq!(normalize_similarity_weight(5.0), 1.0);
        assert_eq!(normalize_similarity_weight(-5.0), 0.0);
    }

    #[test]
    fn cap_similarity_edges_keeps_highest_weight_ties_broken_by_endpoints() {
        let edges = vec![
            edge(1, 2, "similarity", 0.4),
            edge(3, 4, "similarity", 0.9),
            edge(5, 6, "similarity", 0.9),
        ];
        let capped = cap_similarity_edges(edges, 2);
        assert_eq!(
            capped,
            vec![edge(3, 4, "similarity", 0.9), edge(5, 6, "similarity", 0.9)]
        );
    }

    #[test]
    fn cap_similarity_edges_is_a_noop_under_the_cap() {
        let edges = vec![edge(1, 2, "similarity", 0.6)];
        assert_eq!(cap_similarity_edges(edges.clone(), 500), edges);
    }
}
