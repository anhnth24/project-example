//! ACL / role-permission mutation helpers that share the principal authz lock
//! with the upload saga registration path.

use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::db::error::DbError;
use crate::db::orgs;
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
    if n > 0 {
        // 1C-05: this DELETE removes a `role_permissions` row shared by
        // EVERY member holding that role, not just `user_id` — the cache
        // invalidation must be org-wide for the same reason (see
        // migration 0031's doc comment). Skipped when `n == 0` (nothing
        // actually changed) so a no-op revoke does not force every
        // principal in the org to re-resolve for free.
        orgs::bump_acl_version(txn, org_id).await?;
    }
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
    txn.query_one(
        "SELECT 1 FROM collections
             WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL
             FOR NO KEY UPDATE",
        &[&org_id, &collection_id],
    )
    .await
    .map_err(DbError::from)?;
    txn.execute(
        "DELETE FROM collection_group_access
             WHERE org_id = $1 AND collection_id = $2",
        &[&org_id, &collection_id],
    )
    .await
    .map_err(DbError::from)?;
    txn.execute(
        "DELETE FROM collection_role_access
             WHERE org_id = $1 AND collection_id = $2",
        &[&org_id, &collection_id],
    )
    .await
    .map_err(DbError::from)?;
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
    // 1C-05: this changes visibility/ownership org-wide (not just for
    // `user_id`), so the cache invalidation is org-wide too.
    orgs::bump_acl_version(txn, org_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn revoke_collection_access_follows_parent_lock_and_grant_delete_order() {
        let source = include_str!("acl_mutate.rs");
        let lock = source
            .find("FOR NO KEY UPDATE")
            .expect("collection parent lock");
        let delete_group = source
            .find("DELETE FROM collection_group_access")
            .expect("delete group grants");
        let delete_role = source
            .find("DELETE FROM collection_role_access")
            .expect("delete role grants");
        let visibility = source
            .find("SET visibility = 'private'")
            .expect("visibility update");
        let delete_user = source
            .find("DELETE FROM collection_user_access")
            .expect("delete target user grant");
        assert!(
            lock < delete_group
                && delete_group < delete_role
                && delete_role < visibility
                && visibility < delete_user,
            "revoke_collection_access_for_principal must lock parent, delete group/role grants, update visibility, then delete only the target user grant"
        );
    }
}
