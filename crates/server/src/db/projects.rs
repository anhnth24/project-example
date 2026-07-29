//! Tenant-scoped project repository (P2-18).
//!
//! `resolve_project_scope` below is the one entry point `routes::search` and
//! `routes::ask`/`routes::ask::ask_stream_route` both call to turn an
//! optional `projectId` request field into a collection-id filter — see
//! those modules' own docs for why this deliberately feeds the *existing*
//! `collectionIds`/`resolve_scope` retrieval mechanism instead of adding a
//! parallel one.
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

/// Resolves an optional `projectId` request field into an optional
/// collection-id filter, ready to hand to `services::retrieval`'s existing
/// `RetrievalRequest.collection_ids` (which already intersects whatever it
/// is given against `OrgContext::allowed_collection_ids()` — see
/// `services::retrieval::resolve_scope`, unchanged by this feature).
///
/// - `project_id: None` ("all projects") returns `Ok(requested)` unchanged —
///   byte-for-byte today's behavior when no project filter is requested.
/// - `project_id: Some(id)` that does not resolve to a real project in
///   `ctx.org_id()` (never created, or belongs to another org — RLS makes
///   the two indistinguishable, which is the point) returns
///   `Err(DbError::NotFound)`; callers map this straight to a 404, same
///   "no existence oracle" precedent `routes::orgs::get_org` documents.
/// - `project_id: Some(id)` that does resolve returns the intersection of
///   the project's collection ids with `requested` (or just the project's
///   collection ids when `requested` is `None`) — narrowing, never widening,
///   whatever scope the request already specified.
pub async fn resolve_project_scope(
    pool: &Pool,
    ctx: &OrgContext,
    project_id: Option<Uuid>,
    requested: Option<BTreeSet<Uuid>>,
) -> Result<Option<BTreeSet<Uuid>>, DbError> {
    let Some(project_id) = project_id else {
        return Ok(requested);
    };
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                get_by_id(txn, &ctx, project_id).await?;
                let project_collections: BTreeSet<Uuid> =
                    collection_ids_for_project(txn, &ctx, project_id)
                        .await?
                        .into_iter()
                        .collect();
                Ok(match requested {
                    Some(requested) => requested
                        .intersection(&project_collections)
                        .copied()
                        .collect(),
                    None => project_collections,
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
