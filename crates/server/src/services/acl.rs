//! Central ACL / membership services (no HTTP routes yet).
//!
//! Consistent collection readability matches [`crate::db::acl`]: org visibility,
//! owner, direct user ACL, group ACL, and collection-role ACL. Mutations go
//! through [`crate::services::authz_mutation`] barriers.

use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::acl::collection_readable_predicate_c;
use crate::db::authz_epoch;
use crate::db::authz_lock::{LockPool, MutationLockScope};
use crate::db::error::DbError;
use crate::db::models::AccessLevel;
use crate::db::pool::with_org_txn;
use crate::services::authz_mutation::{
    self, grant_collection_group_access, grant_collection_role_access,
    grant_collection_user_access, grant_membership, grant_role_permission,
    revoke_collection_group_access, revoke_collection_role_access, revoke_collection_user_access,
    revoke_membership, revoke_role_permission, update_membership_role, upsert_role,
    AuthzMutationError,
};

/// Resolve allowed collection ids for a user (full ACL surface).
pub async fn resolve_allowed_collections(
    pool: &Pool,
    ctx: &OrgContext,
) -> Result<Vec<Uuid>, DbError> {
    let org_id = ctx.org_id();
    let user_id = ctx.user_id();
    let pred = collection_readable_predicate_c("$2");
    with_org_txn(pool, ctx, move |txn| {
        Box::pin(async move {
            let sql = format!(
                "SELECT c.id
                 FROM collections c
                 WHERE c.org_id = $1
                   AND c.deleted_at IS NULL
                   AND {pred}
                 ORDER BY c.id"
            );
            let rows = txn.query(&sql, &[&org_id, &user_id]).await?;
            Ok(rows.iter().map(|r| r.get(0)).collect())
        })
    })
    .await
}

/// Probe whether `user_id` can read `collection_id` under the consistent ACL.
pub async fn probe_collection_readable(
    pool: &Pool,
    ctx: &OrgContext,
    collection_id: Uuid,
    user_id: Uuid,
) -> Result<bool, DbError> {
    let org_id = ctx.org_id();
    let pred = collection_readable_predicate_c("$3");
    with_org_txn(pool, ctx, move |txn| {
        Box::pin(async move {
            let sql = format!(
                "SELECT EXISTS (
                   SELECT 1 FROM collections c
                   WHERE c.org_id = $1
                     AND c.id = $2
                     AND c.deleted_at IS NULL
                     AND {pred}
                 )"
            );
            let row = txn
                .query_one(&sql, &[&org_id, &collection_id, &user_id])
                .await?;
            Ok(row.get(0))
        })
    })
    .await
}

pub async fn service_grant_membership(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), AuthzMutationError> {
    grant_membership(lock_pool, ctx, org_id, user_id, role).await
}

pub async fn service_revoke_membership(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    revoke_membership(lock_pool, ctx, org_id, user_id).await
}

pub async fn service_update_membership_role(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), AuthzMutationError> {
    update_membership_role(lock_pool, pool, ctx, org_id, user_id, role).await
}

pub async fn service_grant_group_membership(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    group_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    authz_mutation::grant_group_membership(lock_pool, ctx, org_id, group_id, user_id).await
}

pub async fn service_revoke_group_membership(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    group_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    authz_mutation::revoke_group_membership(lock_pool, ctx, org_id, group_id, user_id).await
}

pub async fn service_grant_role_permission(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    role_id: Uuid,
    permission_id: Uuid,
) -> Result<(), AuthzMutationError> {
    grant_role_permission(lock_pool, pool, ctx, org_id, role_id, permission_id).await
}

pub async fn service_revoke_role_permission(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    role_id: Uuid,
    permission_id: Uuid,
) -> Result<(), AuthzMutationError> {
    revoke_role_permission(lock_pool, pool, ctx, org_id, role_id, permission_id).await
}

pub async fn service_upsert_role(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    role_id: Uuid,
    code: &str,
    name: &str,
) -> Result<(), AuthzMutationError> {
    upsert_role(lock_pool, ctx, org_id, role_id, code, name).await
}

pub async fn service_grant_collection_user_access(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), AuthzMutationError> {
    grant_collection_user_access(lock_pool, ctx, org_id, collection_id, user_id, access_level).await
}

pub async fn service_revoke_collection_user_access(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    revoke_collection_user_access(lock_pool, ctx, org_id, collection_id, user_id).await
}

pub async fn service_grant_collection_group_access(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    group_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), AuthzMutationError> {
    grant_collection_group_access(
        lock_pool,
        pool,
        ctx,
        org_id,
        collection_id,
        group_id,
        access_level,
    )
    .await
}

pub async fn service_revoke_collection_group_access(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    group_id: Uuid,
) -> Result<(), AuthzMutationError> {
    revoke_collection_group_access(lock_pool, pool, ctx, org_id, collection_id, group_id).await
}

/// Update collection visibility / owner under collection barrier + impacted users (H7).
pub async fn service_update_collection_visibility_owner(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    visibility: &str,
    owner_user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    if !matches!(visibility, "private" | "org" | "groups") {
        return Err(AuthzMutationError::Invalid("invalid visibility"));
    }
    let visibility = visibility.to_string();
    // Load users with direct/group/role ACL on this collection (+ owner).
    let impacted = with_org_txn(pool, ctx, {
        move |txn| {
            Box::pin(async move {
                let rows = txn
                    .query(
                        "SELECT DISTINCT uid FROM (
                           SELECT owner_user_id AS uid FROM collections
                            WHERE org_id = $1 AND id = $2
                           UNION
                           SELECT user_id FROM collection_user_access
                            WHERE org_id = $1 AND collection_id = $2
                           UNION
                           SELECT gm.user_id FROM collection_group_access cga
                           JOIN group_memberships gm
                             ON gm.org_id = cga.org_id AND gm.group_id = cga.group_id
                            WHERE cga.org_id = $1 AND cga.collection_id = $2
                           UNION
                           SELECT m.user_id FROM collection_role_access cra
                           JOIN roles r ON r.org_id = cra.org_id AND r.id = cra.role_id
                           JOIN org_memberships m
                             ON m.org_id = r.org_id AND m.role = r.code
                            WHERE cra.org_id = $1 AND cra.collection_id = $2
                         ) s
                         WHERE uid IS NOT NULL
                         ORDER BY uid",
                        &[&org_id, &collection_id],
                    )
                    .await?;
                Ok::<_, DbError>(rows.iter().map(|r| r.get::<_, Uuid>(0)).collect::<Vec<_>>())
            })
        }
    })
    .await
    .map_err(AuthzMutationError::from)?;
    let mut users = impacted;
    if !users.contains(&owner_user_id) {
        users.push(owner_user_id);
    }
    let mut scope = MutationLockScope {
        user_ids: users.clone(),
        collection_ids: vec![collection_id],
        ..MutationLockScope::default()
    };
    if !scope.user_ids.contains(&ctx.user_id()) {
        scope.user_ids.push(ctx.user_id());
    }
    crate::db::authz_lock::with_exclusive_mutation_scope_typed(lock_pool, ctx, org_id, &scope, {
        let actor = ctx.user_id();
        let visibility = visibility.clone();
        let users = users.clone();
        move |txn| {
            Box::pin(async move {
                let actor_ok = txn
                    .query_opt(
                        "SELECT 1
                             FROM users u
                             JOIN org_memberships m ON m.user_id = u.id AND m.org_id = $1
                             JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                             JOIN role_permissions rp
                               ON rp.org_id = r.org_id AND rp.role_id = r.id
                             JOIN permissions p ON p.id = rp.permission_id
                             WHERE u.id = $2 AND u.disabled_at IS NULL AND p.code = $3",
                        &[&org_id, &actor, &authz_mutation::PERMISSION_MEMBER_MANAGE],
                    )
                    .await
                    .map_err(DbError::from)?;
                if actor_ok.is_none() {
                    return Err(AuthzMutationError::PermissionDenied);
                }
                let n = txn
                    .execute(
                        "UPDATE collections
                             SET visibility = $3, owner_user_id = $4, updated_at = now()
                             WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL",
                        &[&org_id, &collection_id, &visibility, &owner_user_id],
                    )
                    .await
                    .map_err(DbError::from)?;
                if n == 0 {
                    return Err(AuthzMutationError::NotFound);
                }
                for uid in users {
                    authz_epoch::bump_user_epoch(txn, org_id, uid)
                        .await
                        .map_err(AuthzMutationError::from)?;
                }
                Ok(())
            })
        }
    })
    .await
}

/// Disable a user: target must be current member of acting org; actor fresh enabled+member.manage (H8).
pub async fn service_disable_user(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    authz_mutation::mutate_with_barrier_unchecked_typed(
        lock_pool,
        ctx,
        &[user_id, ctx.user_id()],
        &[],
        {
            let actor = ctx.user_id();
            move |txn| {
                Box::pin(async move {
                    let actor_ok = txn
                        .query_opt(
                            "SELECT 1
                             FROM users u
                             JOIN org_memberships m ON m.user_id = u.id AND m.org_id = $1
                             JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                             JOIN role_permissions rp
                               ON rp.org_id = r.org_id AND rp.role_id = r.id
                             JOIN permissions p ON p.id = rp.permission_id
                             WHERE u.id = $2 AND u.disabled_at IS NULL AND p.code = $3",
                            &[&org_id, &actor, &authz_mutation::PERMISSION_MEMBER_MANAGE],
                        )
                        .await
                        .map_err(DbError::from)?;
                    if actor_ok.is_none() {
                        return Err(AuthzMutationError::PermissionDenied);
                    }
                    let member = txn
                        .query_opt(
                            "SELECT 1 FROM org_memberships
                             WHERE org_id = $1 AND user_id = $2",
                            &[&org_id, &user_id],
                        )
                        .await
                        .map_err(DbError::from)?;
                    if member.is_none() {
                        return Err(AuthzMutationError::NotFound);
                    }
                    txn.execute(
                        "UPDATE users SET disabled_at = coalesce(disabled_at, now()),
                               updated_at = now()
                         WHERE id = $1",
                        &[&user_id],
                    )
                    .await
                    .map_err(DbError::from)?;
                    authz_epoch::bump_user_epoch(txn, org_id, user_id)
                        .await
                        .map_err(AuthzMutationError::from)?;
                    Ok(())
                })
            }
        },
    )
    .await
}

pub async fn service_grant_collection_role_access(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    role_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), AuthzMutationError> {
    grant_collection_role_access(
        lock_pool,
        pool,
        ctx,
        org_id,
        collection_id,
        role_id,
        access_level,
    )
    .await
}

pub async fn service_revoke_collection_role_access(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    role_id: Uuid,
) -> Result<(), AuthzMutationError> {
    revoke_collection_role_access(lock_pool, pool, ctx, org_id, collection_id, role_id).await
}
