//! Tenant-scoped collection repository (ADR 0007).

use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::models::{Collection, CollectionVisibility};

/// Input for creating a collection under the caller's org.
#[derive(Debug, Clone)]
pub struct NewCollection<'a> {
    pub id: Uuid,
    pub name: &'a str,
    pub slug: &'a str,
    pub description: Option<&'a str>,
    pub visibility: CollectionVisibility,
}

/// Inserts a collection for `ctx.org_id` / `ctx.user_id` as owner.
pub async fn insert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewCollection<'_>,
) -> Result<Collection, DbError> {
    let visibility = input.visibility.as_str();
    let row = txn
        .query_one(
            "INSERT INTO collections (
                id, org_id, name, slug, description, owner_user_id, visibility
             ) VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id, org_id, name, slug, description, owner_user_id,
                       visibility, created_at, updated_at, deleted_at",
            &[
                &input.id,
                &ctx.org_id(),
                &input.name,
                &input.slug,
                &input.description,
                &ctx.user_id(),
                &visibility,
            ],
        )
        .await?;
    map_collection(&row)
}

/// Fetches one collection by id within the tenant; cross-org rows are invisible.
pub async fn get_by_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Uuid,
) -> Result<Collection, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, name, slug, description, owner_user_id,
                    visibility, created_at, updated_at, deleted_at
             FROM collections
             WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL",
            &[&ctx.org_id(), &collection_id],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_collection(&row)
}

/// Lists non-deleted collections for the tenant.
pub async fn list(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<Vec<Collection>, DbError> {
    let rows = txn
        .query(
            "SELECT id, org_id, name, slug, description, owner_user_id,
                    visibility, created_at, updated_at, deleted_at
             FROM collections
             WHERE org_id = $1 AND deleted_at IS NULL
             ORDER BY name",
            &[&ctx.org_id()],
        )
        .await?;
    rows.iter().map(map_collection).collect()
}

/// Updates mutable collection fields within the tenant.
pub async fn update_metadata(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Uuid,
    name: &str,
    description: Option<&str>,
) -> Result<Collection, DbError> {
    let row = txn
        .query_opt(
            "UPDATE collections
             SET name = $3,
                 description = $4,
                 updated_at = now()
             WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL
             RETURNING id, org_id, name, slug, description, owner_user_id,
                       visibility, created_at, updated_at, deleted_at",
            &[&ctx.org_id(), &collection_id, &name, &description],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_collection(&row)
}

/// Soft-deletes a collection (tombstone).
pub async fn soft_delete(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    collection_id: Uuid,
) -> Result<(), DbError> {
    let updated = txn
        .execute(
            "UPDATE collections
             SET deleted_at = now(), updated_at = now()
             WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL",
            &[&ctx.org_id(), &collection_id],
        )
        .await?;
    if updated == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

fn map_collection(row: &Row) -> Result<Collection, DbError> {
    let visibility: String = row.get("visibility");
    Ok(Collection {
        id: row.get("id"),
        org_id: row.get("org_id"),
        name: row.get("name"),
        slug: row.get("slug"),
        description: row.get("description"),
        owner_user_id: row.get("owner_user_id"),
        visibility: CollectionVisibility::parse(&visibility).map_err(DbError::Config)?,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        deleted_at: row.get("deleted_at"),
    })
}
