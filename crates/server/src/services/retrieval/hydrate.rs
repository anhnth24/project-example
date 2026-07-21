//! PostgreSQL hydration and ACL/state/version recheck.
//!
//! Qdrant/FTS candidates never become citations until this module authorizes
//! them. Conflict evidence is returned only when both sides remain in scope.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{DocumentState, PublicationState};
use crate::db::pool::with_org_txn;
use crate::db::search::{self, AuthorizedConflictEvidence, HydratedChunkRow, VersionVisibility};
use crate::services::deletion::document_reads_suppressed;

/// Citation-ready chunk after fail-closed recheck.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizedChunk {
    pub chunk_id: Uuid,
    pub chunk_identity_sha256: String,
    pub collection_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub version_number: i32,
    pub content_sha256: String,
    pub heading: String,
    pub body: String,
    pub page: Option<u32>,
    pub slide: Option<u32>,
    pub sheet: Option<String>,
    pub span_start: usize,
    pub span_end: usize,
    pub is_current: bool,
    pub effective_from: DateTime<Utc>,
    pub effective_to: Option<DateTime<Utc>>,
}

/// Fail-closed gate applied to every hydrated row (hermetic / unit-tested).
pub fn authorize_hydrated_row(
    ctx: &OrgContext,
    row: &HydratedChunkRow,
    visibility: &VersionVisibility,
) -> Option<AuthorizedChunk> {
    if row.org_id != ctx.org_id() {
        return None;
    }
    if !ctx.allows_collection(row.collection_id) {
        return None;
    }
    if document_reads_suppressed(row.document_state, row.deleted_at.is_some()) {
        return None;
    }
    if row.document_state != DocumentState::Indexed {
        return None;
    }
    if row.publication_state != PublicationState::Published {
        return None;
    }
    match visibility {
        VersionVisibility::Current => {
            if !row.is_current {
                return None;
            }
        }
        VersionVisibility::VersionIds(allowed) => {
            if !allowed.contains(&row.version_id) {
                return None;
            }
        }
    }
    let heading = row.heading_path.join(" / ");
    let span_start = row.span_start.unwrap_or(0).max(0) as usize;
    let span_end = row
        .span_end
        .map(|value| value.max(0) as usize)
        .unwrap_or(row.body.len())
        .max(span_start);
    Some(AuthorizedChunk {
        chunk_id: row.chunk_id,
        chunk_identity_sha256: row.chunk_identity_sha256.clone(),
        collection_id: row.collection_id,
        document_id: row.document_id,
        version_id: row.version_id,
        version_number: row.version_number,
        content_sha256: row.content_sha256.clone(),
        heading,
        body: row.body.clone(),
        page: row.page.and_then(|value| u32::try_from(value).ok()),
        slide: row.slide.and_then(|value| u32::try_from(value).ok()),
        sheet: row.sheet.clone(),
        span_start,
        span_end,
        is_current: row.is_current,
        effective_from: row.effective_from,
        effective_to: row.effective_to,
    })
}

/// Hydrates identities and drops unauthorized/stale rows.
pub async fn hydrate_authorized_chunks(
    pool: &Pool,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    identities: &[String],
    visibility: &VersionVisibility,
) -> Result<HashMap<String, AuthorizedChunk>, DbError> {
    let visibility = visibility.clone();
    let collection_ids = collection_ids.to_vec();
    let identities = identities.to_vec();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let rows = search::hydrate_chunks_by_identity(
                    txn,
                    &ctx,
                    &collection_ids,
                    &identities,
                    &visibility,
                )
                .await?;
                let mut out = HashMap::new();
                for row in rows {
                    if let Some(authorized) = authorize_hydrated_row(&ctx, &row, &visibility) {
                        out.insert(authorized.chunk_identity_sha256.clone(), authorized);
                    }
                }
                Ok(out)
            })
        }
    })
    .await
}

/// Conflict evidence is returned only when both claim sides stay authorized.
pub fn both_sides_authorized(ctx: &OrgContext, evidence: &AuthorizedConflictEvidence) -> bool {
    ctx.allows_collection(evidence.claim_a_collection_id)
        && ctx.allows_collection(evidence.claim_b_collection_id)
}

/// Loads conflict evidence and keeps pairs where both sides remain in scope.
pub async fn hydrate_authorized_conflict_evidence(
    pool: &Pool,
    ctx: &OrgContext,
    collection_ids: &[Uuid],
    conflict_ids: &[Uuid],
) -> Result<Vec<AuthorizedConflictEvidence>, DbError> {
    let collection_ids = collection_ids.to_vec();
    let conflict_ids = conflict_ids.to_vec();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let rows = search::load_authorized_conflict_evidence(
                    txn,
                    &ctx,
                    &collection_ids,
                    &conflict_ids,
                )
                .await?;
                Ok(rows
                    .into_iter()
                    .filter(|row| both_sides_authorized(&ctx, row))
                    .collect())
            })
        }
    })
    .await
}

/// Stale vector payloads must not become text without a hydrated row.
pub fn text_only_from_hydration<'a>(
    identity: &str,
    hydrated: &'a HashMap<String, AuthorizedChunk>,
) -> Option<&'a AuthorizedChunk> {
    hydrated.get(identity)
}

/// Collects unique identities from lexical + vector legs for hydration.
pub fn collect_candidate_identities(
    lexical: impl IntoIterator<Item = String>,
    vector: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    lexical.into_iter().chain(vector).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_row(collection_id: Uuid, is_current: bool) -> HydratedChunkRow {
        HydratedChunkRow {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "identity".into(),
            org_id: Uuid::new_v4(),
            collection_id,
            document_id: Uuid::new_v4(),
            version_id: Uuid::new_v4(),
            version_number: 1,
            content_sha256: "a".repeat(64),
            heading_path: vec!["Heading".into()],
            body: "body text".into(),
            page: Some(1),
            slide: None,
            sheet: None,
            span_start: Some(0),
            span_end: Some(4),
            document_state: DocumentState::Indexed,
            deleted_at: None,
            publication_state: PublicationState::Published,
            is_current,
            effective_from: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            effective_to: None,
        }
    }

    #[test]
    fn stale_vector_without_hydration_yields_no_text() {
        let hydrated = HashMap::new();
        assert!(text_only_from_hydration("missing", &hydrated).is_none());
    }

    #[test]
    fn cross_scope_and_tombstone_denied() {
        let collection = Uuid::new_v4();
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let ctx = OrgContext::try_new(org, user, ["qa.query"], [collection]).unwrap();
        let mut row = sample_row(collection, true);
        row.org_id = org;
        assert!(authorize_hydrated_row(&ctx, &row, &VersionVisibility::Current).is_some());

        row.collection_id = Uuid::new_v4();
        assert!(authorize_hydrated_row(&ctx, &row, &VersionVisibility::Current).is_none());

        row.collection_id = collection;
        row.deleted_at = Some(Utc::now());
        row.document_state = DocumentState::Tombstoned;
        assert!(authorize_hydrated_row(&ctx, &row, &VersionVisibility::Current).is_none());
    }

    #[test]
    fn current_mode_rejects_superseded_version() {
        let collection = Uuid::new_v4();
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let ctx = OrgContext::try_new(org, user, ["qa.query"], [collection]).unwrap();
        let mut row = sample_row(collection, false);
        row.org_id = org;
        assert!(authorize_hydrated_row(&ctx, &row, &VersionVisibility::Current).is_none());
    }

    #[test]
    fn conflict_requires_both_authorized_collections() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let ctx = OrgContext::try_new(org, user, ["qa.query"], [a]).unwrap();
        let evidence = AuthorizedConflictEvidence {
            conflict_id: Uuid::new_v4(),
            claim_a_id: Uuid::new_v4(),
            claim_b_id: Uuid::new_v4(),
            claim_a_document_id: Uuid::new_v4(),
            claim_b_document_id: Uuid::new_v4(),
            claim_a_version_id: Uuid::new_v4(),
            claim_b_version_id: Uuid::new_v4(),
            claim_a_collection_id: a,
            claim_b_collection_id: b,
            claim_a_quote: Some("left".into()),
            claim_b_quote: Some("right".into()),
        };
        assert!(!both_sides_authorized(&ctx, &evidence));
        let ctx_both = OrgContext::try_new(org, user, ["qa.query"], [a, b]).unwrap();
        assert!(both_sides_authorized(&ctx_both, &evidence));
    }
}
