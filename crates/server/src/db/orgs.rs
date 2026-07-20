//! Org lookup scoped by [`OrgContext`] (global `orgs` table, still fail-closed).

use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::Org;

/// Returns the organization named by `ctx.org_id()` only.
pub async fn get(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<Org, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, slug, name, created_at, updated_at
             FROM orgs
             WHERE id = $1",
            &[&ctx.org_id()],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_org(&row)
}

/// Ensures an org row exists for `ctx` (used by tests / bootstrap under context).
pub async fn ensure_exists(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    slug: &str,
    name: &str,
) -> Result<Org, DbError> {
    txn.execute(
        "INSERT INTO orgs (id, slug, name)
         VALUES ($1, $2, $3)
         ON CONFLICT (id) DO NOTHING",
        &[&ctx.org_id(), &slug, &name],
    )
    .await?;
    get(txn, ctx).await
}

fn map_org(row: &Row) -> Result<Org, DbError> {
    Ok(Org {
        id: row.get("id"),
        slug: row.get("slug"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

/// Inserts a user if missing (users are global; still requires OrgContext for call sites).
pub async fn ensure_user(
    txn: &Transaction<'_>,
    _ctx: &OrgContext,
    user_id: Uuid,
    email: &str,
    display_name: &str,
) -> Result<(), DbError> {
    txn.execute(
        "INSERT INTO users (id, email, display_name)
         VALUES ($1, $2, $3)
         ON CONFLICT (id) DO NOTHING",
        &[&user_id, &email, &display_name],
    )
    .await?;
    Ok(())
}

/// Ensures membership for `ctx.user_id` in `ctx.org_id` (RLS-scoped insert).
pub async fn ensure_membership(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<(), DbError> {
    txn.execute(
        "INSERT INTO org_memberships (org_id, user_id, role)
         VALUES ($1, $2, 'owner')
         ON CONFLICT (org_id, user_id) DO NOTHING",
        &[&ctx.org_id(), &ctx.user_id()],
    )
    .await?;
    Ok(())
}
