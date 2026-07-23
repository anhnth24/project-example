//! ACL / role-permission mutation helpers that share the principal authz lock
//! with the upload saga registration path.

use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::db::error::DbError;
use crate::services::authz_lock;

/// Revoke a permission code from every role held by `user_id` in `org_id`.
///
/// Takes the shared principal authz advisory lock first so registration cannot
/// observe a torn permission set.
pub async fn revoke_role_permission_for_principal(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
    permission_code: &str,
) -> Result<u64, DbError> {
    authz_lock::lock_principal_authz(txn, org_id, user_id).await?;
    let n = txn
        .execute(
            "DELETE FROM role_permissions rp
             USING roles r, org_memberships m, permissions p
             WHERE rp.org_id = $1
               AND rp.role_id = r.id
               AND r.org_id = m.org_id
               AND r.code = m.role
               AND m.org_id = $1
               AND m.user_id = $2
               AND rp.permission_id = p.id
               AND p.code = $3",
            &[&org_id, &user_id, &permission_code],
        )
        .await
        .map_err(DbError::from)?;
    Ok(n)
}

/// Deny collection access for a principal: transfer ownership away (if needed),
/// set private visibility, and drop `collection_user_access` rows.
pub async fn revoke_collection_access_for_principal(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
    collection_id: Uuid,
    new_owner_user_id: Uuid,
) -> Result<(), DbError> {
    authz_lock::lock_principal_authz(txn, org_id, user_id).await?;
    txn.execute(
        "UPDATE collections
         SET visibility = 'private',
             owner_user_id = $3,
             updated_at = now()
         WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL",
        &[&org_id, &collection_id, &new_owner_user_id],
    )
    .await
    .map_err(DbError::from)?;
    txn.execute(
        "DELETE FROM collection_user_access
         WHERE org_id = $1 AND collection_id = $2 AND user_id = $3",
        &[&org_id, &collection_id, &user_id],
    )
    .await
    .map_err(DbError::from)?;
    Ok(())
}
