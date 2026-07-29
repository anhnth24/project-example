//! Document Graph MVP (P2-17) read queries: visible-document nodes,
//! conflict-derived edges, and co-citation edges. Every query is tenant- and
//! ACL-scoped the same way `services::access`'s conflict queries are (dual
//! collection-leg authorization), so a caller only ever sees nodes/edges
//! built from documents they could already list.

use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

#[derive(Debug, Clone, PartialEq)]
pub struct GraphDocumentRow {
    pub id: Uuid,
    pub title: String,
    pub collection_id: Uuid,
    pub collection_name: String,
    pub state: String,
}

/// Visible documents for the graph: same ACL as `documents::list_in_collection`
/// (org + `ctx.allowed_collection_ids()`), optionally narrowed to one
/// collection. `fetch_cap` is a safety bound on the query itself — the
/// route applies the contract's degree-based `prune()` on top of whatever
/// this returns, so `fetch_cap` should be generous relative to the final
/// node cap (pruning, not query truncation, decides which nodes survive).
pub async fn list_visible_documents(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_filter: Option<Uuid>,
    fetch_cap: i64,
) -> Result<Vec<GraphDocumentRow>, DbError> {
    if let Some(collection_id) = collection_filter {
        if !ctx.allowed_collection_ids().contains(&collection_id) {
            return Err(DbError::Config("collection_denied".into()));
        }
    }
    let allowed: Vec<Uuid> = ctx.allowed_collection_ids().iter().copied().collect();
    let fetch_cap = fetch_cap.clamp(1, 10_000);
    let rows = txn
        .query(
            "SELECT d.id, d.title, d.collection_id, c.name AS collection_name, d.state
             FROM documents d
             JOIN collections c ON c.org_id = d.org_id AND c.id = d.collection_id
             WHERE d.org_id = $1
               AND d.collection_id = ANY($2::uuid[])
               AND ($3::uuid IS NULL OR d.collection_id = $3)
               AND d.deleted_at IS NULL
               AND d.state NOT IN ('tombstoned', 'purged')
             ORDER BY d.created_at DESC, d.id DESC
             LIMIT $4",
            &[&ctx.org_id(), &allowed, &collection_filter, &fetch_cap],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| GraphDocumentRow {
            id: row.get("id"),
            title: row.get("title"),
            collection_id: row.get("collection_id"),
            collection_name: row.get("collection_name"),
            state: row.get("state"),
        })
        .collect())
}

/// `conflict` edges: open conflicts whose two claims belong to two documents
/// both present in `node_ids` (already ACL-filtered by the caller — this
/// query does not re-check collection ACL beyond `org_id`, matching
/// `services::access::list_authorized_conflicts`'s own dual-leg join, which
/// this mirrors). Self-pairs (a conflict whose two claims land on the same
/// document) are excluded — a graph edge needs two distinct nodes.
///
/// Returns `(document_a_id, document_b_id, open_conflict_count)`, one row
/// per distinct document pair.
pub async fn conflict_edges_among(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    node_ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, i64)>, DbError> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT da.id AS document_a_id, db.id AS document_b_id, count(*)::bigint AS cnt
             FROM conflicts conf
             JOIN claims ca ON ca.org_id = conf.org_id AND ca.id = conf.claim_a_id
             JOIN claims cb ON cb.org_id = conf.org_id AND cb.id = conf.claim_b_id
             JOIN documents da ON da.org_id = ca.org_id AND da.id = ca.document_id
             JOIN documents db ON db.org_id = cb.org_id AND db.id = cb.document_id
             WHERE conf.org_id = $1
               AND conf.status = 'open'
               AND da.id = ANY($2::uuid[])
               AND db.id = ANY($2::uuid[])
               AND da.id <> db.id
             GROUP BY da.id, db.id",
            &[&ctx.org_id(), &node_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| {
            (
                row.get("document_a_id"),
                row.get("document_b_id"),
                row.get("cnt"),
            )
        })
        .collect())
}

/// `co_citation` edges: two documents were both cited by the same grounded
/// answer. Backed by `ask_stream_sessions.cited_document_ids` — the only
/// place the codebase durably records "which documents this answer cited"
/// (see `services/qa/ask_stream.rs`); there is no separate per-answer
/// citation history table. Citations are only ever written into that column
/// after `qa.query`-gated authorization
/// (`stream_auth::revalidate_ask_stream`/`fence_principal_and_citations`),
/// so aggregating across sessions here does not leak anything a caller
/// couldn't already see — this query still restricts to `node_ids`
/// (already ACL-filtered) as a second, redundant guard.
///
/// Returns `(document_a_id, document_b_id, distinct_session_count)` for
/// document pairs with `document_a_id < document_b_id`.
pub async fn co_citation_edges_among(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    node_ids: &[Uuid],
) -> Result<Vec<(Uuid, Uuid, i64)>, DbError> {
    if node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "WITH distinct_pairs AS (
                SELECT DISTINCT s.id AS session_id, a.doc AS doc_a, b.doc AS doc_b
                FROM ask_stream_sessions s
                CROSS JOIN LATERAL unnest(s.cited_document_ids) AS a(doc)
                CROSS JOIN LATERAL unnest(s.cited_document_ids) AS b(doc)
                WHERE s.org_id = $1
                  AND a.doc < b.doc
                  AND a.doc = ANY($2::uuid[])
                  AND b.doc = ANY($2::uuid[])
             )
             SELECT doc_a, doc_b, count(*)::bigint AS cnt
             FROM distinct_pairs
             GROUP BY doc_a, doc_b",
            &[&ctx.org_id(), &node_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| (row.get("doc_a"), row.get("doc_b"), row.get("cnt")))
        .collect())
}
