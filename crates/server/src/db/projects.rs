//! Tenant-scoped project repository (P2-18; multi-project `projectIds[]`
//! added P2-19).
//!
//! `resolve_project_scope` below is the one entry point `routes::search` and
//! `routes::ask`/`routes::ask::ask_stream_route` both call to turn the
//! caller's project scope (the deprecated singular `projectId` and/or the
//! `projectIds[]` array, merged by [`merge_project_ids`]) into a
//! collection-id filter — see those modules' own docs for why this
//! deliberately feeds the *existing* `collectionIds`/`resolve_scope`
//! retrieval mechanism instead of adding a parallel one.
//!
//! A project is a named, org-scoped folder of collections
//! (migrations/0032) — see `db::collections::assign_project` for the
//! collection side of the relationship. There is no soft-delete here yet;
//! project deletion is explicitly out of scope for this slice (see the
//! P2-18 backlog entry) precisely to avoid deciding orphaned-collection
//! semantics prematurely.

use std::collections::BTreeSet;

use deadpool_postgres::Pool;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::Project;
use crate::db::pool::with_org_txn;

const PROJECT_COLUMNS: &str = "id, org_id, name, created_at, updated_at";

/// Inserts a project for `ctx.org_id()`.
pub async fn insert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    id: Uuid,
    name: &str,
) -> Result<Project, DbError> {
    let row = txn
        .query_one(
            &format!(
                "INSERT INTO projects (id, org_id, name)
             VALUES ($1, $2, $3)
             RETURNING {PROJECT_COLUMNS}"
            ),
            &[&id, &ctx.org_id(), &name],
        )
        .await?;
    Ok(map_project(&row))
}

/// Fetches one project by id within the tenant; cross-org rows are invisible
/// (RLS also enforces this — this WHERE clause is defense-in-depth, same
/// convention as `db::collections::get_by_id`).
pub async fn get_by_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    project_id: Uuid,
) -> Result<Project, DbError> {
    let row = txn
        .query_opt(
            &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE org_id = $1 AND id = $2"),
            &[&ctx.org_id(), &project_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(map_project(&row))
}

/// Lists every project for the tenant, alphabetically.
pub async fn list(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<Vec<Project>, DbError> {
    let rows = txn
        .query(
            &format!("SELECT {PROJECT_COLUMNS} FROM projects WHERE org_id = $1 ORDER BY name"),
            &[&ctx.org_id()],
        )
        .await?;
    Ok(rows.iter().map(map_project).collect())
}

/// Renames a project within the tenant.
pub async fn update_name(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    project_id: Uuid,
    name: &str,
) -> Result<Project, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE projects
             SET name = $3, updated_at = now()
             WHERE org_id = $1 AND id = $2
             RETURNING {PROJECT_COLUMNS}"
            ),
            &[&ctx.org_id(), &project_id, &name],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(map_project(&row))
}

/// Collection ids belonging to `project_id`, scoped to the tenant and
/// excluding soft-deleted collections — the exact filter every other
/// collection read in this codebase applies (`db::collections::list`).
/// Fail-closed: an empty result (unknown project, or a real project with
/// zero collections) is a valid, non-error outcome — callers decide what an
/// empty scope means (see `routes::search`/`routes::ask`'s project filter).
pub async fn collection_ids_for_project(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    project_id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let rows = txn
        .query(
            "SELECT id FROM collections
             WHERE org_id = $1 AND project_id = $2 AND deleted_at IS NULL",
            &[&ctx.org_id(), &project_id],
        )
        .await?;
    Ok(rows.iter().map(|row| row.get(0)).collect())
}

/// Bound on the number of `projectIds` accepted per request (P2-19) — the
/// route layer rejects a longer array with 400 via [`merge_project_ids`]
/// before [`resolve_project_scope`] ever runs (and therefore before any
/// database round trip).
pub const MAX_PROJECT_IDS: usize = 20;

/// Pure, DB-free merge of the deprecated singular `projectId` and the new
/// `projectIds[]` request fields (P2-19) into one bounded id list ready for
/// [`resolve_project_scope`]. `routes::search`/`routes::ask` both call this
/// before touching the database so the 400 (too many ids) never costs a
/// round trip. Order/duplicates in the result are irrelevant —
/// `resolve_project_scope` de-duplicates into a `BTreeSet` internally.
pub fn merge_project_ids(
    project_id: Option<Uuid>,
    project_ids: Option<Vec<Uuid>>,
) -> Result<Vec<Uuid>, &'static str> {
    let many = project_ids.unwrap_or_default();
    if many.len() > MAX_PROJECT_IDS {
        return Err("Too many projectIds");
    }
    let mut merged: Vec<Uuid> = project_id.into_iter().collect();
    merged.extend(many);
    Ok(merged)
}

/// Resolves zero, one, or many project ids (already merged by
/// [`merge_project_ids`]) into an optional collection-id filter, ready to
/// hand to `services::retrieval`'s existing `RetrievalRequest.collection_ids`
/// (which already intersects whatever it is given against
/// `OrgContext::allowed_collection_ids()` — see
/// `services::retrieval::resolve_scope`, unchanged by this feature).
///
/// - `project_ids: []` ("all projects" — true whether the caller sent
///   neither field, an empty `projectIds`, or both empty/absent) returns
///   `Ok(requested)` unchanged — byte-for-byte today's behavior when no
///   project filter is requested.
/// - Any id in `project_ids` that does not resolve to a real project in
///   `ctx.org_id()` (never created, or belongs to another org — RLS makes
///   the two indistinguishable, which is the point) returns
///   `Err(DbError::NotFound)`; callers map this straight to a 404, same
///   "no existence oracle" precedent `routes::orgs::get_org` documents. This
///   applies to every id in the array, not just a first/only one — one bad
///   id anywhere in `projectIds` 404s the whole request, matching the
///   single-`projectId` contract's semantics id-for-id.
/// - When every id resolves, the result is the *union* of every named
///   project's collection ids, intersected with `requested` (or returned
///   as-is when `requested` is `None`) — still narrowing, never widening,
///   whatever scope the request already specified. `projectId` and
///   `projectIds` given together are unioned the same way (P2-19): they are
///   just two sources feeding the same id list by the time this runs.
pub async fn resolve_project_scope(
    pool: &Pool,
    ctx: &OrgContext,
    project_ids: &[Uuid],
    requested: Option<BTreeSet<Uuid>>,
) -> Result<Option<BTreeSet<Uuid>>, DbError> {
    if project_ids.is_empty() {
        return Ok(requested);
    }
    let ids: BTreeSet<Uuid> = project_ids.iter().copied().collect();
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let mut union: BTreeSet<Uuid> = BTreeSet::new();
                for project_id in &ids {
                    get_by_id(txn, &ctx, *project_id).await?;
                    union.extend(collection_ids_for_project(txn, &ctx, *project_id).await?);
                }
                Ok(match requested {
                    Some(requested) => requested.intersection(&union).copied().collect(),
                    None => union,
                })
            })
        }
    })
    .await
    .map(Some)
}

fn map_project(row: &Row) -> Project {
    Project {
        id: row.get("id"),
        org_id: row.get("org_id"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn merge_absent_both_is_empty() {
        assert_eq!(merge_project_ids(None, None).unwrap(), Vec::<Uuid>::new());
    }

    #[test]
    fn merge_empty_array_same_as_absent() {
        assert_eq!(
            merge_project_ids(None, Some(Vec::new())).unwrap(),
            Vec::<Uuid>::new()
        );
    }

    #[test]
    fn merge_unions_singular_and_array() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let c = Uuid::new_v4();
        let merged = merge_project_ids(Some(a), Some(vec![b, c])).unwrap();
        let set: BTreeSet<Uuid> = merged.into_iter().collect();
        assert_eq!(set, BTreeSet::from([a, b, c]));
    }

    #[test]
    fn merge_rejects_more_than_max() {
        let ids: Vec<Uuid> = (0..(MAX_PROJECT_IDS + 1)).map(|_| Uuid::new_v4()).collect();
        assert!(merge_project_ids(None, Some(ids)).is_err());
    }

    #[test]
    fn merge_accepts_exactly_max() {
        let ids: Vec<Uuid> = (0..MAX_PROJECT_IDS).map(|_| Uuid::new_v4()).collect();
        assert!(merge_project_ids(None, Some(ids)).is_ok());
    }
}
