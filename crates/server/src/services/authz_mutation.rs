//! Central authorization-sensitive mutation API (Q&A delivery barrier).
//!
//! All membership/role/ACL mutations that can invalidate an in-flight Q&A stream
//! take exclusive advisory locks (writer intent + affected user keys), re-resolve
//! the actor's `member.manage` **inside** the locked txn, and bump epochs before
//! commit. Callers should use these APIs instead of raw SQL against membership/ACL
//! tables where avoidable.

use deadpool_postgres::Pool;
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::authz_epoch;
use crate::db::authz_lock::{
    with_exclusive_mutation_scope_typed, with_exclusive_mutation_users,
    with_exclusive_mutation_users_typed, LockPool, MutationLockScope,
};
use crate::db::error::DbError;
use crate::db::models::AccessLevel;
use crate::db::pool::OrgTxnFuture;

/// Admin permission required for membership / ACL / role mutations.
pub const PERMISSION_MEMBER_MANAGE: &str = "member.manage";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationTarget {
    /// Membership / role / ACL change for one user (user lock).
    User { user_id: Uuid },
    /// Document delete/publish/ACL-affecting change.
    Documents {
        document_ids: Vec<Uuid>,
        user_id: Uuid,
    },
}

impl MutationTarget {
    pub fn user_id(&self) -> Uuid {
        match self {
            Self::User { user_id } => *user_id,
            Self::Documents { user_id, .. } => *user_id,
        }
    }

    pub fn document_ids(&self) -> &[Uuid] {
        match self {
            Self::User { .. } => &[],
            Self::Documents { document_ids, .. } => document_ids,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthzMutationError {
    #[error("permission denied")]
    PermissionDenied,
    #[error("database error")]
    Database,
    #[error("lock timeout")]
    LockTimeout,
    #[error("not found")]
    NotFound,
    #[error("invalid request: {0}")]
    Invalid(&'static str),
}

impl From<DbError> for AuthzMutationError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::LockTimeout | DbError::WriterIntent => Self::LockTimeout,
            DbError::NotFound => Self::NotFound,
            DbError::Config(msg) if msg.contains("not found") => Self::NotFound,
            _ => Self::Database,
        }
    }
}

/// Re-resolve actor membership + `member.manage` on the locked transaction.
async fn require_actor_member_manage_in_txn(
    txn: &Transaction<'_>,
    org_id: Uuid,
    actor_user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    let user = txn
        .query_opt(
            "SELECT disabled_at FROM users WHERE id = $1",
            &[&actor_user_id],
        )
        .await
        .map_err(DbError::from)?;
    let Some(user) = user else {
        return Err(AuthzMutationError::PermissionDenied);
    };
    let disabled_at: Option<chrono::DateTime<chrono::Utc>> = user.get(0);
    if disabled_at.is_some() {
        return Err(AuthzMutationError::PermissionDenied);
    }
    let row = txn
        .query_opt(
            "SELECT 1
             FROM org_memberships m
             JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
             JOIN role_permissions rp ON rp.org_id = r.org_id AND rp.role_id = r.id
             JOIN permissions p ON p.id = rp.permission_id
             WHERE m.org_id = $1 AND m.user_id = $2 AND p.code = $3",
            &[&org_id, &actor_user_id, &PERMISSION_MEMBER_MANAGE],
        )
        .await
        .map_err(DbError::from)?;
    if row.is_none() {
        return Err(AuthzMutationError::PermissionDenied);
    }
    Ok(())
}

/// Low-level exclusive barrier **without** actor re-resolution.
///
/// Internal / document-publish paths only. Prefer the typed membership APIs
/// which re-check `member.manage` inside the locked txn.
pub(crate) async fn mutate_with_barrier_unchecked<T, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    user_ids: &[Uuid],
    document_ids: &[Uuid],
    f: F,
) -> Result<T, DbError>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> OrgTxnFuture<'c, T>,
{
    with_exclusive_mutation_users(lock_pool, ctx, ctx.org_id(), user_ids, document_ids, f).await
}

/// Typed unchecked barrier (deletion/promotion). Internal use.
pub(crate) async fn mutate_with_barrier_unchecked_typed<T, E, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    user_ids: &[Uuid],
    document_ids: &[Uuid],
    f: F,
) -> Result<T, E>
where
    F: for<'c> FnOnce(
        &'c Transaction<'c>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'c>,
    >,
    E: From<DbError>,
{
    with_exclusive_mutation_users_typed(lock_pool, ctx, ctx.org_id(), user_ids, document_ids, f)
        .await
}

/// Admin mutation: lock scope, re-resolve `member.manage` inside txn, then `f`.
async fn mutate_admin_scope<T, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    mut scope: MutationLockScope,
    f: F,
) -> Result<T, AuthzMutationError>
where
    F: for<'c> FnOnce(
            &'c Transaction<'c>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T, AuthzMutationError>> + Send + 'c>,
        > + Send
        + 'static,
{
    let org_id = ctx.org_id();
    let actor = ctx.user_id();
    if !scope.user_ids.contains(&actor) {
        scope.user_ids.push(actor);
    }
    with_exclusive_mutation_scope_typed(lock_pool, ctx, org_id, &scope, move |txn| {
        Box::pin(async move {
            require_actor_member_manage_in_txn(txn, org_id, actor).await?;
            f(txn).await
        })
    })
    .await
}

async fn mutate_admin<T, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    user_ids: &[Uuid],
    document_ids: &[Uuid],
    f: F,
) -> Result<T, AuthzMutationError>
where
    F: for<'c> FnOnce(
            &'c Transaction<'c>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<T, AuthzMutationError>> + Send + 'c>,
        > + Send
        + 'static,
{
    mutate_admin_scope(
        lock_pool,
        ctx,
        MutationLockScope {
            user_ids: user_ids.to_vec(),
            document_ids: document_ids.to_vec(),
            ..MutationLockScope::default()
        },
        f,
    )
    .await
}

const MEMBER_SET_RETRIES: usize = 4;

/// Compatibility wrapper used by deletion/promotion (single user + docs).
pub async fn mutate_with_barrier<T, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    target: MutationTarget,
    f: F,
) -> Result<T, DbError>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> OrgTxnFuture<'c, T>,
{
    // Unchecked: callers are document lifecycle paths, not admin ACL APIs.
    mutate_with_barrier_unchecked(
        lock_pool,
        ctx,
        &[target.user_id()],
        target.document_ids(),
        f,
    )
    .await
}

/// Typed barrier for service errors that implement `From<DbError>`.
pub async fn mutate_with_barrier_typed<T, E, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    target: MutationTarget,
    f: F,
) -> Result<T, E>
where
    F: for<'c> FnOnce(
        &'c Transaction<'c>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'c>,
    >,
    E: From<DbError>,
{
    mutate_with_barrier_unchecked_typed(
        lock_pool,
        ctx,
        &[target.user_id()],
        target.document_ids(),
        f,
    )
    .await
}

/// Grant org membership under exclusive barrier + user epoch bump.
pub async fn grant_membership(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), AuthzMutationError> {
    if role.trim().is_empty() {
        return Err(AuthzMutationError::Invalid("role required"));
    }
    let role = role.to_string();
    mutate_admin(lock_pool, ctx, &[user_id], &[], move |txn| {
        Box::pin(async move {
            authz_epoch::bump_user_epoch(txn, org_id, user_id)
                .await
                .map_err(AuthzMutationError::from)?;
            txn.execute(
                "INSERT INTO org_memberships (org_id, user_id, role)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (org_id, user_id) DO UPDATE SET role = EXCLUDED.role",
                &[&org_id, &user_id, &role],
            )
            .await
            .map_err(DbError::from)?;
            Ok(())
        })
    })
    .await
}

/// Revoke org membership under exclusive barrier + user epoch bump.
pub async fn revoke_membership(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    mutate_admin(lock_pool, ctx, &[user_id], &[], move |txn| {
        Box::pin(async move {
            authz_epoch::bump_user_epoch(txn, org_id, user_id)
                .await
                .map_err(AuthzMutationError::from)?;
            let n = txn
                .execute(
                    "DELETE FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                    &[&org_id, &user_id],
                )
                .await
                .map_err(DbError::from)?;
            if n == 0 {
                return Err(AuthzMutationError::NotFound);
            }
            Ok(())
        })
    })
    .await
}

/// Update membership role under exclusive barrier + user epoch bump.
///
/// Acquires stable lock keys for both the previous and new role rows (when
/// present) so concurrent role-ACL mutations cannot race the assignment.
pub async fn update_membership_role(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), AuthzMutationError> {
    if role.trim().is_empty() {
        return Err(AuthzMutationError::Invalid("role required"));
    }
    let role = role.to_string();
    let (old_role_id, new_role_id) = crate::db::pool::with_org_txn(pool, ctx, {
        let role = role.clone();
        move |txn| {
            Box::pin(async move {
                let old: Option<Uuid> = txn
                    .query_opt(
                        "SELECT r.id
                         FROM org_memberships m
                         JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                         WHERE m.org_id = $1 AND m.user_id = $2",
                        &[&org_id, &user_id],
                    )
                    .await?
                    .map(|row| row.get(0));
                let new_id: Option<Uuid> = txn
                    .query_opt(
                        "SELECT id FROM roles WHERE org_id = $1 AND code = $2",
                        &[&org_id, &role],
                    )
                    .await?
                    .map(|row| row.get(0));
                Ok::<_, DbError>((old, new_id))
            })
        }
    })
    .await
    .map_err(AuthzMutationError::from)?;
    let mut role_ids = Vec::new();
    if let Some(id) = old_role_id {
        role_ids.push(id);
    }
    if let Some(id) = new_role_id {
        if !role_ids.contains(&id) {
            role_ids.push(id);
        }
    }
    mutate_admin_scope(
        lock_pool,
        ctx,
        MutationLockScope {
            user_ids: vec![user_id],
            role_ids,
            ..MutationLockScope::default()
        },
        move |txn| {
            Box::pin(async move {
                authz_epoch::bump_user_epoch(txn, org_id, user_id)
                    .await
                    .map_err(AuthzMutationError::from)?;
                let n = txn
                    .execute(
                        "UPDATE org_memberships SET role = $3
                         WHERE org_id = $1 AND user_id = $2",
                        &[&org_id, &user_id, &role],
                    )
                    .await
                    .map_err(DbError::from)?;
                if n == 0 {
                    return Err(AuthzMutationError::NotFound);
                }
                Ok(())
            })
        },
    )
    .await
}

/// Grant collection user ACL (`access_level` required).
pub async fn grant_collection_user_access(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), AuthzMutationError> {
    let level = access_level.as_str().to_string();
    mutate_admin(lock_pool, ctx, &[user_id], &[], move |txn| {
        Box::pin(async move {
            authz_epoch::bump_user_epoch(txn, org_id, user_id)
                .await
                .map_err(AuthzMutationError::from)?;
            txn.execute(
                "INSERT INTO collection_user_access (
                    org_id, collection_id, user_id, access_level
                 ) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (collection_id, user_id) DO UPDATE
                   SET access_level = EXCLUDED.access_level",
                &[&org_id, &collection_id, &user_id, &level],
            )
            .await
            .map_err(DbError::from)?;
            Ok(())
        })
    })
    .await
}

/// Revoke collection user ACL under exclusive barrier + user epoch bump.
pub async fn revoke_collection_user_access(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    mutate_admin(lock_pool, ctx, &[user_id], &[], move |txn| {
        Box::pin(async move {
            authz_epoch::bump_user_epoch(txn, org_id, user_id)
                .await
                .map_err(AuthzMutationError::from)?;
            txn.execute(
                "DELETE FROM collection_user_access
                 WHERE org_id = $1 AND collection_id = $2 AND user_id = $3",
                &[&org_id, &collection_id, &user_id],
            )
            .await
            .map_err(DbError::from)?;
            Ok(())
        })
    })
    .await
}

async fn load_group_member_ids(
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    group_id: Uuid,
) -> Result<Vec<Uuid>, AuthzMutationError> {
    crate::db::pool::with_org_txn(pool, ctx, move |txn| {
        Box::pin(async move {
            let rows = txn
                .query(
                    "SELECT user_id FROM group_memberships
                     WHERE org_id = $1 AND group_id = $2
                     ORDER BY user_id",
                    &[&org_id, &group_id],
                )
                .await?;
            Ok(rows.iter().map(|r| r.get(0)).collect())
        })
    })
    .await
    .map_err(AuthzMutationError::from)
}

async fn load_role_member_ids(
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    role_id: Uuid,
) -> Result<Vec<Uuid>, AuthzMutationError> {
    crate::db::pool::with_org_txn(pool, ctx, move |txn| {
        Box::pin(async move {
            let rows = txn
                .query(
                    "SELECT m.user_id
                     FROM org_memberships m
                     JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                     WHERE m.org_id = $1 AND r.id = $2
                     ORDER BY m.user_id",
                    &[&org_id, &role_id],
                )
                .await?;
            Ok(rows.iter().map(|r| r.get(0)).collect())
        })
    })
    .await
    .map_err(AuthzMutationError::from)
}

/// Grant collection group ACL (`access_level` required).
///
/// H5: acquires stable group key; reloads member set, compares, retries on drift.
pub async fn grant_collection_group_access(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    group_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), AuthzMutationError> {
    let level = access_level.as_str().to_string();
    for _ in 0..MEMBER_SET_RETRIES {
        let members = load_group_member_ids(pool, ctx, org_id, group_id).await?;
        let expected = members.clone();
        let outcome = mutate_admin_scope(
            lock_pool,
            ctx,
            MutationLockScope {
                user_ids: members,
                collection_ids: vec![collection_id],
                group_ids: vec![group_id],
                ..MutationLockScope::default()
            },
            {
                let level = level.clone();
                let expected = expected.clone();
                move |txn| {
                    Box::pin(async move {
                        let fresh_rows = txn
                            .query(
                                "SELECT user_id FROM group_memberships
                                 WHERE org_id = $1 AND group_id = $2
                                 ORDER BY user_id",
                                &[&org_id, &group_id],
                            )
                            .await
                            .map_err(DbError::from)?;
                        let fresh: Vec<Uuid> = fresh_rows.iter().map(|r| r.get(0)).collect();
                        if fresh != expected {
                            return Err(AuthzMutationError::Invalid("member set changed"));
                        }
                        txn.execute(
                            "INSERT INTO collection_group_access (
                                org_id, collection_id, group_id, access_level
                             ) VALUES ($1, $2, $3, $4)
                             ON CONFLICT (collection_id, group_id) DO UPDATE
                               SET access_level = EXCLUDED.access_level",
                            &[&org_id, &collection_id, &group_id, &level],
                        )
                        .await
                        .map_err(DbError::from)?;
                        for uid in fresh {
                            authz_epoch::bump_user_epoch(txn, org_id, uid)
                                .await
                                .map_err(AuthzMutationError::from)?;
                        }
                        Ok(())
                    })
                }
            },
        )
        .await;
        match outcome {
            Err(AuthzMutationError::Invalid("member set changed")) => continue,
            other => return other,
        }
    }
    Err(AuthzMutationError::LockTimeout)
}

/// Revoke collection group ACL (stable group key + member-set retry).
pub async fn revoke_collection_group_access(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    group_id: Uuid,
) -> Result<(), AuthzMutationError> {
    for _ in 0..MEMBER_SET_RETRIES {
        let members = load_group_member_ids(pool, ctx, org_id, group_id).await?;
        let expected = members.clone();
        let outcome = mutate_admin_scope(
            lock_pool,
            ctx,
            MutationLockScope {
                user_ids: members,
                collection_ids: vec![collection_id],
                group_ids: vec![group_id],
                ..MutationLockScope::default()
            },
            {
                let expected = expected.clone();
                move |txn| {
                    Box::pin(async move {
                        let fresh_rows = txn
                            .query(
                                "SELECT user_id FROM group_memberships
                                 WHERE org_id = $1 AND group_id = $2
                                 ORDER BY user_id",
                                &[&org_id, &group_id],
                            )
                            .await
                            .map_err(DbError::from)?;
                        let fresh: Vec<Uuid> = fresh_rows.iter().map(|r| r.get(0)).collect();
                        if fresh != expected {
                            return Err(AuthzMutationError::Invalid("member set changed"));
                        }
                        txn.execute(
                            "DELETE FROM collection_group_access
                             WHERE org_id = $1 AND collection_id = $2 AND group_id = $3",
                            &[&org_id, &collection_id, &group_id],
                        )
                        .await
                        .map_err(DbError::from)?;
                        for uid in fresh {
                            authz_epoch::bump_user_epoch(txn, org_id, uid)
                                .await
                                .map_err(AuthzMutationError::from)?;
                        }
                        Ok(())
                    })
                }
            },
        )
        .await;
        match outcome {
            Err(AuthzMutationError::Invalid("member set changed")) => continue,
            other => return other,
        }
    }
    Err(AuthzMutationError::LockTimeout)
}

/// Upsert an org role (non-system by default).
pub async fn upsert_role(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    role_id: Uuid,
    code: &str,
    name: &str,
) -> Result<(), AuthzMutationError> {
    if code.trim().is_empty() || name.trim().is_empty() {
        return Err(AuthzMutationError::Invalid("role code/name required"));
    }
    let code = code.to_string();
    let name = name.to_string();
    mutate_admin(lock_pool, ctx, &[], &[], move |txn| {
        Box::pin(async move {
            txn.execute(
                "INSERT INTO roles (id, org_id, code, name, is_system)
                 VALUES ($1, $2, $3, $4, false)
                 ON CONFLICT (org_id, code) DO UPDATE
                   SET name = EXCLUDED.name, updated_at = now()
                   WHERE roles.is_system = false",
                &[&role_id, &org_id, &code, &name],
            )
            .await
            .map_err(DbError::from)?;
            Ok(())
        })
    })
    .await
}

/// Grant a permission to a role; stable role key + member-set retry.
pub async fn grant_role_permission(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    role_id: Uuid,
    permission_id: Uuid,
) -> Result<(), AuthzMutationError> {
    for _ in 0..MEMBER_SET_RETRIES {
        let members = load_role_member_ids(pool, ctx, org_id, role_id).await?;
        let expected = members.clone();
        let outcome = mutate_admin_scope(
            lock_pool,
            ctx,
            MutationLockScope {
                user_ids: members,
                role_ids: vec![role_id],
                ..MutationLockScope::default()
            },
            {
                let expected = expected.clone();
                move |txn| {
                    Box::pin(async move {
                        let fresh_rows = txn
                            .query(
                                "SELECT m.user_id
                                 FROM org_memberships m
                                 JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                                 WHERE m.org_id = $1 AND r.id = $2
                                 ORDER BY m.user_id",
                                &[&org_id, &role_id],
                            )
                            .await
                            .map_err(DbError::from)?;
                        let fresh: Vec<Uuid> = fresh_rows.iter().map(|r| r.get(0)).collect();
                        if fresh != expected {
                            return Err(AuthzMutationError::Invalid("member set changed"));
                        }
                        txn.execute(
                            "INSERT INTO role_permissions (org_id, role_id, permission_id)
                             VALUES ($1, $2, $3)
                             ON CONFLICT DO NOTHING",
                            &[&org_id, &role_id, &permission_id],
                        )
                        .await
                        .map_err(DbError::from)?;
                        for uid in fresh {
                            authz_epoch::bump_user_epoch(txn, org_id, uid)
                                .await
                                .map_err(AuthzMutationError::from)?;
                        }
                        Ok(())
                    })
                }
            },
        )
        .await;
        match outcome {
            Err(AuthzMutationError::Invalid("member set changed")) => continue,
            other => return other,
        }
    }
    Err(AuthzMutationError::LockTimeout)
}

/// Revoke a permission from a role; stable role key + member-set retry.
pub async fn revoke_role_permission(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    role_id: Uuid,
    permission_id: Uuid,
) -> Result<(), AuthzMutationError> {
    for _ in 0..MEMBER_SET_RETRIES {
        let members = load_role_member_ids(pool, ctx, org_id, role_id).await?;
        let expected = members.clone();
        let outcome = mutate_admin_scope(
            lock_pool,
            ctx,
            MutationLockScope {
                user_ids: members,
                role_ids: vec![role_id],
                ..MutationLockScope::default()
            },
            {
                let expected = expected.clone();
                move |txn| {
                    Box::pin(async move {
                        let fresh_rows = txn
                            .query(
                                "SELECT m.user_id
                                 FROM org_memberships m
                                 JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                                 WHERE m.org_id = $1 AND r.id = $2
                                 ORDER BY m.user_id",
                                &[&org_id, &role_id],
                            )
                            .await
                            .map_err(DbError::from)?;
                        let fresh: Vec<Uuid> = fresh_rows.iter().map(|r| r.get(0)).collect();
                        if fresh != expected {
                            return Err(AuthzMutationError::Invalid("member set changed"));
                        }
                        txn.execute(
                            "DELETE FROM role_permissions
                             WHERE org_id = $1 AND role_id = $2 AND permission_id = $3",
                            &[&org_id, &role_id, &permission_id],
                        )
                        .await
                        .map_err(DbError::from)?;
                        for uid in fresh {
                            authz_epoch::bump_user_epoch(txn, org_id, uid)
                                .await
                                .map_err(AuthzMutationError::from)?;
                        }
                        Ok(())
                    })
                }
            },
        )
        .await;
        match outcome {
            Err(AuthzMutationError::Invalid("member set changed")) => continue,
            other => return other,
        }
    }
    Err(AuthzMutationError::LockTimeout)
}

/// Grant group membership under `mutate_admin_scope` (enabled actor + stable group).
pub async fn grant_group_membership(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    group_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    mutate_admin_scope(
        lock_pool,
        ctx,
        MutationLockScope {
            user_ids: vec![user_id],
            group_ids: vec![group_id],
            ..MutationLockScope::default()
        },
        move |txn| {
            Box::pin(async move {
                let target_ok = txn
                    .query_opt(
                        "SELECT 1 FROM users WHERE id = $1 AND disabled_at IS NULL",
                        &[&user_id],
                    )
                    .await
                    .map_err(DbError::from)?;
                if target_ok.is_none() {
                    return Err(AuthzMutationError::NotFound);
                }
                txn.execute(
                    "INSERT INTO group_memberships (org_id, group_id, user_id)
                     VALUES ($1, $2, $3)
                     ON CONFLICT DO NOTHING",
                    &[&org_id, &group_id, &user_id],
                )
                .await
                .map_err(DbError::from)?;
                authz_epoch::bump_user_epoch(txn, org_id, user_id)
                    .await
                    .map_err(AuthzMutationError::from)?;
                Ok(())
            })
        },
    )
    .await
}

/// Revoke group membership under `mutate_admin_scope` (enabled actor + stable group).
pub async fn revoke_group_membership(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    group_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    mutate_admin_scope(
        lock_pool,
        ctx,
        MutationLockScope {
            user_ids: vec![user_id],
            group_ids: vec![group_id],
            ..MutationLockScope::default()
        },
        move |txn| {
            Box::pin(async move {
                txn.execute(
                    "DELETE FROM group_memberships
                     WHERE org_id = $1 AND group_id = $2 AND user_id = $3",
                    &[&org_id, &group_id, &user_id],
                )
                .await
                .map_err(DbError::from)?;
                authz_epoch::bump_user_epoch(txn, org_id, user_id)
                    .await
                    .map_err(AuthzMutationError::from)?;
                Ok(())
            })
        },
    )
    .await
}

/// Backward-compatible aliases.
pub async fn grant_collection_access(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    grant_collection_user_access(
        lock_pool,
        ctx,
        org_id,
        collection_id,
        user_id,
        AccessLevel::Read,
    )
    .await
}

pub async fn revoke_collection_access(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    user_id: Uuid,
) -> Result<(), AuthzMutationError> {
    revoke_collection_user_access(lock_pool, ctx, org_id, collection_id, user_id).await
}

/// Grant collection-role ACL (H6).
pub async fn grant_collection_role_access(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    role_id: Uuid,
    access_level: AccessLevel,
) -> Result<(), AuthzMutationError> {
    let level = access_level.as_str().to_string();
    for _ in 0..MEMBER_SET_RETRIES {
        let members = load_role_member_ids(pool, ctx, org_id, role_id).await?;
        let expected = members.clone();
        let outcome = mutate_admin_scope(
            lock_pool,
            ctx,
            MutationLockScope {
                user_ids: members,
                collection_ids: vec![collection_id],
                role_ids: vec![role_id],
                ..MutationLockScope::default()
            },
            {
                let level = level.clone();
                let expected = expected.clone();
                move |txn| {
                    Box::pin(async move {
                        let fresh_rows = txn
                            .query(
                                "SELECT m.user_id
                                 FROM org_memberships m
                                 JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                                 WHERE m.org_id = $1 AND r.id = $2
                                 ORDER BY m.user_id",
                                &[&org_id, &role_id],
                            )
                            .await
                            .map_err(DbError::from)?;
                        let fresh: Vec<Uuid> = fresh_rows.iter().map(|r| r.get(0)).collect();
                        if fresh != expected {
                            return Err(AuthzMutationError::Invalid("member set changed"));
                        }
                        txn.execute(
                            "INSERT INTO collection_role_access (
                                org_id, collection_id, role_id, access_level
                             ) VALUES ($1, $2, $3, $4)
                             ON CONFLICT (collection_id, role_id) DO UPDATE
                               SET access_level = EXCLUDED.access_level",
                            &[&org_id, &collection_id, &role_id, &level],
                        )
                        .await
                        .map_err(DbError::from)?;
                        for uid in fresh {
                            authz_epoch::bump_user_epoch(txn, org_id, uid)
                                .await
                                .map_err(AuthzMutationError::from)?;
                        }
                        Ok(())
                    })
                }
            },
        )
        .await;
        match outcome {
            Err(AuthzMutationError::Invalid("member set changed")) => continue,
            other => return other,
        }
    }
    Err(AuthzMutationError::LockTimeout)
}

/// Revoke collection-role ACL (H6).
pub async fn revoke_collection_role_access(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    org_id: Uuid,
    collection_id: Uuid,
    role_id: Uuid,
) -> Result<(), AuthzMutationError> {
    for _ in 0..MEMBER_SET_RETRIES {
        let members = load_role_member_ids(pool, ctx, org_id, role_id).await?;
        let expected = members.clone();
        let outcome = mutate_admin_scope(
            lock_pool,
            ctx,
            MutationLockScope {
                user_ids: members,
                collection_ids: vec![collection_id],
                role_ids: vec![role_id],
                ..MutationLockScope::default()
            },
            {
                let expected = expected.clone();
                move |txn| {
                    Box::pin(async move {
                        let fresh_rows = txn
                            .query(
                                "SELECT m.user_id
                                 FROM org_memberships m
                                 JOIN roles r ON r.org_id = m.org_id AND r.code = m.role
                                 WHERE m.org_id = $1 AND r.id = $2
                                 ORDER BY m.user_id",
                                &[&org_id, &role_id],
                            )
                            .await
                            .map_err(DbError::from)?;
                        let fresh: Vec<Uuid> = fresh_rows.iter().map(|r| r.get(0)).collect();
                        if fresh != expected {
                            return Err(AuthzMutationError::Invalid("member set changed"));
                        }
                        txn.execute(
                            "DELETE FROM collection_role_access
                             WHERE org_id = $1 AND collection_id = $2 AND role_id = $3",
                            &[&org_id, &collection_id, &role_id],
                        )
                        .await
                        .map_err(DbError::from)?;
                        for uid in fresh {
                            authz_epoch::bump_user_epoch(txn, org_id, uid)
                                .await
                                .map_err(AuthzMutationError::from)?;
                        }
                        Ok(())
                    })
                }
            },
        )
        .await;
        match outcome {
            Err(AuthzMutationError::Invalid("member set changed")) => continue,
            other => return other,
        }
    }
    Err(AuthzMutationError::LockTimeout)
}

/// Bump document epoch after publish pointer swap (caller holds exclusive txn).
pub async fn bump_document_epoch_in_txn(
    txn: &Transaction<'_>,
    org_id: Uuid,
    document_id: Uuid,
) -> Result<(), DbError> {
    authz_epoch::bump_document_epoch(txn, org_id, document_id).await?;
    Ok(())
}

/// Helper unused marker so `Pool` import stays available for dual-pool APIs.
#[allow(dead_code)]
fn _pool_ty(_: &Pool) {}
