//! Fail-closed FTS visibility diagnostics for integration search timeouts.
//!
//! Mirrors production `fts_search` / `hydrate_chunks_by_identity` / ACL SQL without
//! logging marker text, runtime ids, or chunk bodies.

use deadpool_postgres::Pool;
use fileconv_server::auth::context::OrgContext;
use fileconv_server::auth::permissions::resolve_org_context_in_txn;
use fileconv_server::db::acl_sql::acl_predicate_sql;
use fileconv_server::db::error::DbError;
use fileconv_server::db::models::AccessLevel;
use fileconv_server::db::pool::with_org_txn;
use fileconv_server::db::search::{
    self, normalized_fts_query_for_retrieval, normalized_fts_query_stats, VersionVisibility,
};
use fileconv_server::services::retrieval::{
    leg_candidate_limit_for_request, resolve_scope, RetrievalError,
};
use tokio_postgres::Transaction;
use uuid::Uuid;

/// Sanitized production-path predicate summary (no query text or runtime ids).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtsVisibilitySnapshotParts {
    pub normalized_token_count: usize,
    pub normalized_query_nonempty: bool,
    pub allowed_collection_count: usize,
    pub document_collection_in_allowed: bool,
    pub resolve_scope_collection_count: usize,
    pub document_collection_in_scope: bool,
    pub lexical_candidate_limit: usize,
    pub lexical_candidate_count: usize,
    pub hydration_row_count: usize,
    pub target_document_in_lexical_candidates: bool,
    pub target_document_in_hydration: bool,
    pub target_chunk_count: usize,
    pub target_any_tsv_match_raw: bool,
    pub target_any_tsv_match_normalized: bool,
    pub target_all_acl_predicate_pass: bool,
    pub target_all_structural_predicates_pass: bool,
}

pub fn format_fts_visibility_snapshot(parts: &FtsVisibilitySnapshotParts) -> String {
    [
        format!("normalized_token_count={}", parts.normalized_token_count),
        format!(
            "normalized_query_nonempty={}",
            parts.normalized_query_nonempty
        ),
        format!(
            "allowed_collection_count={}",
            parts.allowed_collection_count
        ),
        format!(
            "document_collection_in_allowed={}",
            parts.document_collection_in_allowed
        ),
        format!(
            "resolve_scope_collection_count={}",
            parts.resolve_scope_collection_count
        ),
        format!(
            "document_collection_in_scope={}",
            parts.document_collection_in_scope
        ),
        format!("lexical_candidate_limit={}", parts.lexical_candidate_limit),
        format!("lexical_candidate_count={}", parts.lexical_candidate_count),
        format!("hydration_row_count={}", parts.hydration_row_count),
        format!(
            "target_document_in_lexical_candidates={}",
            parts.target_document_in_lexical_candidates
        ),
        format!(
            "target_document_in_hydration={}",
            parts.target_document_in_hydration
        ),
        format!("target_chunk_count={}", parts.target_chunk_count),
        format!(
            "target_any_tsv_match_raw={}",
            parts.target_any_tsv_match_raw
        ),
        format!(
            "target_any_tsv_match_normalized={}",
            parts.target_any_tsv_match_normalized
        ),
        format!(
            "target_all_acl_predicate_pass={}",
            parts.target_all_acl_predicate_pass
        ),
        format!(
            "target_all_structural_predicates_pass={}",
            parts.target_all_structural_predicates_pass
        ),
    ]
    .join("; ")
}

struct TargetDocumentChunkAggregate {
    chunk_count: i64,
    any_tsv_match_raw: bool,
    any_tsv_match_normalized: bool,
    all_acl_predicate_pass: bool,
    all_structural_predicates_pass: bool,
}

/// Mirrors production `fts_search` structural filters for `VersionVisibility::Current`.
fn target_chunk_structural_predicates_expr() -> &'static str {
    "d.deleted_at IS NULL
     AND d.state = 'indexed'
     AND dv.publication_state = 'published'
     AND dv.is_current
     AND im.is_active
     AND im.state = 'active'"
}

async fn load_target_document_chunk_aggregate(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    raw_query: &str,
    normalized_query: &str,
) -> Result<TargetDocumentChunkAggregate, DbError> {
    let permission = "qa.query";
    let access = AccessLevel::Read.as_str();
    let acl = acl_predicate_sql("d.org_id", "d.collection_id", "$4", "$5", "$6");
    let structural = target_chunk_structural_predicates_expr();
    let sql = format!(
        "SELECT COUNT(*)::bigint AS chunk_count,
                COALESCE(BOOL_OR(c.tsv @@ plainto_tsquery('simple', $3)), false)
                    AS any_tsv_match_raw,
                COALESCE(BOOL_OR(c.tsv @@ plainto_tsquery('simple', $7)), false)
                    AS any_tsv_match_normalized,
                COALESCE(BOOL_AND({acl}), false) AS all_acl_predicate_pass,
                COALESCE(BOOL_AND(
                    {structural}
                ), false) AS all_structural_predicates_pass
         FROM chunks c
         JOIN documents d
           ON d.org_id = c.org_id AND d.id = c.document_id
         JOIN document_versions dv
           ON dv.org_id = c.org_id
          AND dv.document_id = c.document_id
          AND dv.id = c.version_id
         JOIN index_metadata im
           ON im.org_id = c.org_id AND im.id = c.index_metadata_id
         WHERE c.org_id = $1 AND c.document_id = $2",
        structural = structural,
    );
    let row = txn
        .query_one(
            &sql,
            &[
                &ctx.org_id(),
                &document_id,
                &raw_query,
                &ctx.user_id(),
                &permission,
                &access,
                &normalized_query,
            ],
        )
        .await?;
    Ok(TargetDocumentChunkAggregate {
        chunk_count: row.get(0),
        any_tsv_match_raw: row.get(1),
        any_tsv_match_normalized: row.get(2),
        all_acl_predicate_pass: row.get(3),
        all_structural_predicates_pass: row.get(4),
    })
}

async fn collect_fts_visibility_snapshot_parts_on_txn(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    target_document_id: Uuid,
    document_collection_id: Uuid,
    query: &str,
    request_limit: usize,
) -> Result<FtsVisibilitySnapshotParts, DbError> {
    let query_stats = normalized_fts_query_stats(query);
    let normalized_query = normalized_fts_query_for_retrieval(query);
    let lexical_candidate_limit = leg_candidate_limit_for_request(request_limit);
    let visibility = VersionVisibility::Current;

    let lexical = search::fts_search(
        txn,
        ctx,
        collection_ids,
        query,
        &visibility,
        lexical_candidate_limit,
    )
    .await?;
    let target_document_in_lexical_candidates = lexical
        .iter()
        .any(|candidate| candidate.document_id == target_document_id);

    let identities: Vec<String> = lexical
        .iter()
        .map(|candidate| candidate.chunk_identity_sha256.clone())
        .collect();
    let hydration = if identities.is_empty() {
        Vec::new()
    } else {
        search::hydrate_chunks_by_identity(txn, ctx, collection_ids, &identities, &visibility)
            .await?
    };
    let target_document_in_hydration = hydration
        .iter()
        .any(|row| row.document_id == target_document_id);

    let aggregate = load_target_document_chunk_aggregate(
        txn,
        ctx,
        target_document_id,
        query,
        &normalized_query,
    )
    .await?;

    let allowed = ctx.allowed_collection_ids();
    Ok(FtsVisibilitySnapshotParts {
        normalized_token_count: query_stats.token_count,
        normalized_query_nonempty: query_stats.nonempty,
        allowed_collection_count: allowed.len(),
        document_collection_in_allowed: allowed.contains(&document_collection_id),
        resolve_scope_collection_count: collection_ids.len(),
        document_collection_in_scope: collection_ids.contains(&document_collection_id),
        lexical_candidate_limit,
        lexical_candidate_count: lexical.len(),
        hydration_row_count: hydration.len(),
        target_document_in_lexical_candidates,
        target_document_in_hydration,
        target_chunk_count: usize::try_from(aggregate.chunk_count.max(0)).unwrap_or(0),
        target_any_tsv_match_raw: aggregate.any_tsv_match_raw,
        target_any_tsv_match_normalized: aggregate.any_tsv_match_normalized,
        target_all_acl_predicate_pass: aggregate.all_acl_predicate_pass,
        target_all_structural_predicates_pass: aggregate.all_structural_predicates_pass,
    })
}

/// Builds a sanitized production-path FTS visibility snapshot for one org/document.
pub async fn search_visibility_snapshot_for_document(
    pool: &Pool,
    org_id: Uuid,
    owner_user_id: Uuid,
    target_document_id: Uuid,
    document_collection_id: Uuid,
    query: &str,
    request_limit: usize,
) -> Result<String, String> {
    let ctx = resolve_org_context_in_txn(pool, org_id, owner_user_id)
        .await
        .map_err(|err| format!("resolve org context: {err:?}"))?;
    let scope = resolve_scope(&ctx, None)
        .map_err(|err: RetrievalError| format!("resolve scope: {err:?}"))?;
    let collection_ids: Vec<_> = scope.collection_ids.iter().copied().collect();

    let parts = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        let query = query.to_string();
        move |txn| {
            Box::pin(async move {
                collect_fts_visibility_snapshot_parts_on_txn(
                    txn,
                    &ctx,
                    &collection_ids,
                    target_document_id,
                    document_collection_id,
                    &query,
                    request_limit,
                )
                .await
            })
        }
    })
    .await
    .map_err(|err| format!("collect fts visibility snapshot: {err}"))?;

    Ok(format_fts_visibility_snapshot(&parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fileconv_server::services::retrieval::RETRIEVAL_LEG_CANDIDATE_LIMIT;
    use uuid::Uuid;

    fn sample_parts() -> FtsVisibilitySnapshotParts {
        FtsVisibilitySnapshotParts {
            normalized_token_count: 4,
            normalized_query_nonempty: true,
            allowed_collection_count: 3,
            document_collection_in_allowed: true,
            resolve_scope_collection_count: 3,
            document_collection_in_scope: true,
            lexical_candidate_limit: RETRIEVAL_LEG_CANDIDATE_LIMIT,
            lexical_candidate_count: 0,
            hydration_row_count: 0,
            target_document_in_lexical_candidates: false,
            target_document_in_hydration: false,
            target_chunk_count: 1,
            target_any_tsv_match_raw: true,
            target_any_tsv_match_normalized: false,
            target_all_acl_predicate_pass: false,
            target_all_structural_predicates_pass: true,
        }
    }

    #[test]
    fn formatted_snapshot_includes_required_field_names() {
        let rendered = format_fts_visibility_snapshot(&sample_parts());
        for needle in [
            "normalized_token_count=",
            "normalized_query_nonempty=",
            "allowed_collection_count=",
            "document_collection_in_allowed=",
            "resolve_scope_collection_count=",
            "document_collection_in_scope=",
            "lexical_candidate_limit=",
            "lexical_candidate_count=",
            "hydration_row_count=",
            "target_document_in_lexical_candidates=",
            "target_document_in_hydration=",
            "target_chunk_count=",
            "target_any_tsv_match_raw=",
            "target_any_tsv_match_normalized=",
            "target_all_acl_predicate_pass=",
            "target_all_structural_predicates_pass=",
        ] {
            assert!(
                rendered.contains(needle),
                "snapshot missing {needle:?}: {rendered}"
            );
        }
    }

    #[test]
    fn formatted_snapshot_omits_marker_query_and_runtime_ids() {
        let marker = format!("phase1c-marker-alpha-{}", "a".repeat(32));
        let document_id = Uuid::new_v4();
        let chunk_id = Uuid::new_v4();
        let rendered = format_fts_visibility_snapshot(&sample_parts());
        for forbidden in [
            marker.as_str(),
            "phase1c-marker-alpha",
            "SUPERSECRET123",
            &document_id.to_string(),
            &chunk_id.to_string(),
        ] {
            assert!(
                !rendered.contains(forbidden),
                "snapshot leaked forbidden value {forbidden:?}: {rendered}"
            );
        }
    }

    #[test]
    fn lexical_candidate_limit_matches_production_helper() {
        assert_eq!(
            leg_candidate_limit_for_request(10),
            RETRIEVAL_LEG_CANDIDATE_LIMIT
        );
        assert_eq!(leg_candidate_limit_for_request(300), 300);
    }

    #[test]
    fn target_structural_predicates_match_production_fts_search_current() {
        let expr = target_chunk_structural_predicates_expr();
        for needle in [
            "d.deleted_at IS NULL",
            "d.state = 'indexed'",
            "dv.publication_state = 'published'",
            "dv.is_current",
            "im.is_active",
            "im.state = 'active'",
        ] {
            assert!(
                expr.contains(needle),
                "structural predicate missing {needle:?}: {expr}"
            );
        }
    }
}
