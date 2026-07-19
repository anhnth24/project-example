//! Resolve current-state OrgContext from PostgreSQL (JWT claims are hints only).

use deadpool_postgres::Client;
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

/// Failures when resolving authorization from current PG membership state.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("user is disabled")]
    UserDisabled,
    #[error("org membership not found")]
    MembershipMissing,
    #[error("permission denied")]
    PermissionDenied,
    #[error("collection access denied")]
    CollectionDenied,
    #[error("org context construction failed")]
    InvalidContext,
    #[error("database error")]
    Database,
}

impl From<DbError> for ResolveError {
    fn from(_: DbError) -> Self {
        Self::Database
    }
}

/// Loads permissions and allowed collections from current PG state.
///
/// Requires `app.org_id` (and preferably `app.user_id`) already set on `client`
/// when querying RLS-protected tables. Callers typically use [`crate::db::pool::with_org_txn`]
/// with a provisional context, or set GUCs before calling.
pub async fn resolve_org_context(
    client: &Client,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<OrgContext, ResolveError> {
    let user_row = client
        .query_opt("SELECT disabled_at FROM users WHERE id = $1", &[&user_id])
        .await
        .map_err(|_| ResolveError::Database)?;
    let Some(user_row) = user_row else {
        return Err(ResolveError::MembershipMissing);
    };
    let disabled_at: Option<chrono::DateTime<chrono::Utc>> = user_row.get(0);
    if disabled_at.is_some() {
        return Err(ResolveError::UserDisabled);
    }

    let membership = client
        .query_opt(
            "SELECT role FROM org_memberships WHERE org_id = $1 AND user_id = $2",
            &[&org_id, &user_id],
        )
        .await
        .map_err(|_| ResolveError::Database)?;
    if membership.is_none() {
        return Err(ResolveError::MembershipMissing);
    }

    let permission_rows = client
        .query(
            "SELECT p.code
             FROM org_memberships m
             JOIN roles r
               ON r.org_id = m.org_id AND r.code = m.role
             JOIN role_permissions rp
               ON rp.org_id = r.org_id AND rp.role_id = r.id
             JOIN permissions p
               ON p.id = rp.permission_id
             WHERE m.org_id = $1 AND m.user_id = $2
             ORDER BY p.code",
            &[&org_id, &user_id],
        )
        .await
        .map_err(|_| ResolveError::Database)?;
    let permissions: Vec<String> = permission_rows.iter().map(|row| row.get(0)).collect();

    // POC collection allow-list (full ACL is Phase 1C): org-visible, owned, or direct user grant.
    let collection_rows = client
        .query(
            "SELECT c.id
             FROM collections c
             WHERE c.org_id = $1
               AND c.deleted_at IS NULL
               AND (
                 c.visibility = 'org'
                 OR c.owner_user_id = $2
                 OR EXISTS (
                   SELECT 1 FROM collection_user_access cua
                   WHERE cua.org_id = c.org_id
                     AND cua.collection_id = c.id
                     AND cua.user_id = $2
                 )
               )",
            &[&org_id, &user_id],
        )
        .await
        .map_err(|_| ResolveError::Database)?;
    let collections: Vec<Uuid> = collection_rows.iter().map(|row| row.get(0)).collect();

    OrgContext::try_new(org_id, user_id, permissions, collections)
        .map_err(|_| ResolveError::InvalidContext)
}

/// Returns true when `ctx` holds the named permission code.
pub fn check_permission(ctx: &OrgContext, code: &str) -> bool {
    ctx.has_permission(code)
}

/// Fail-closed permission gate for business routes.
pub fn require_permission(ctx: &OrgContext, code: &str) -> Result<(), ResolveError> {
    if check_permission(ctx, code) {
        Ok(())
    } else {
        Err(ResolveError::PermissionDenied)
    }
}

/// Fail-closed collection allow-list gate for business routes.
pub fn require_collection(ctx: &OrgContext, collection_id: Uuid) -> Result<(), ResolveError> {
    if ctx.allows_collection(collection_id) {
        Ok(())
    } else {
        Err(ResolveError::CollectionDenied)
    }
}

/// Convenience: resolve inside a transaction-local org GUC scope.
pub async fn resolve_org_context_in_txn(
    pool: &deadpool_postgres::Pool,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<OrgContext, ResolveError> {
    // Provisional context only to set RLS GUCs; real permissions come from the query below.
    let provisional = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
        .map_err(|_| ResolveError::InvalidContext)?;
    crate::db::pool::with_org_txn(pool, &provisional, move |txn| {
        Box::pin(async move {
            // Re-run resolution using the transaction connection via raw queries.
            let user_row = txn
                .query_opt("SELECT disabled_at FROM users WHERE id = $1", &[&user_id])
                .await?;
            let Some(user_row) = user_row else {
                return Err(DbError::NotFound);
            };
            let disabled_at: Option<chrono::DateTime<chrono::Utc>> = user_row.get(0);
            if disabled_at.is_some() {
                return Err(DbError::Config("user_disabled".into()));
            }
            let membership = txn
                .query_opt(
                    "SELECT role FROM org_memberships WHERE org_id = $1 AND user_id = $2",
                    &[&org_id, &user_id],
                )
                .await?;
            if membership.is_none() {
                return Err(DbError::NotFound);
            }
            let permission_rows = txn
                .query(
                    "SELECT p.code
                     FROM org_memberships m
                     JOIN roles r
                       ON r.org_id = m.org_id AND r.code = m.role
                     JOIN role_permissions rp
                       ON rp.org_id = r.org_id AND rp.role_id = r.id
                     JOIN permissions p
                       ON p.id = rp.permission_id
                     WHERE m.org_id = $1 AND m.user_id = $2
                     ORDER BY p.code",
                    &[&org_id, &user_id],
                )
                .await?;
            let permissions: Vec<String> = permission_rows.iter().map(|row| row.get(0)).collect();
            let collection_rows = txn
                .query(
                    "SELECT c.id
                     FROM collections c
                     WHERE c.org_id = $1
                       AND c.deleted_at IS NULL
                       AND (
                         c.visibility = 'org'
                         OR c.owner_user_id = $2
                         OR EXISTS (
                           SELECT 1 FROM collection_user_access cua
                           WHERE cua.org_id = c.org_id
                             AND cua.collection_id = c.id
                             AND cua.user_id = $2
                         )
                       )",
                    &[&org_id, &user_id],
                )
                .await?;
            let collections: Vec<Uuid> = collection_rows.iter().map(|row| row.get(0)).collect();
            OrgContext::try_new(org_id, user_id, permissions, collections)
                .map_err(|_| DbError::Config("invalid_org_context".into()))
        })
    })
    .await
    .map_err(|error| match error {
        DbError::NotFound => ResolveError::MembershipMissing,
        DbError::Config(ref msg) if msg == "user_disabled" => ResolveError::UserDisabled,
        _ => ResolveError::Database,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_helpers_fail_closed() {
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let collection = Uuid::new_v4();
        let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection]).unwrap();
        assert!(check_permission(&ctx, "doc.upload"));
        assert!(require_permission(&ctx, "doc.upload").is_ok());
        assert_eq!(
            require_permission(&ctx, "doc.delete"),
            Err(ResolveError::PermissionDenied)
        );
        assert!(require_collection(&ctx, collection).is_ok());
        assert_eq!(
            require_collection(&ctx, Uuid::new_v4()),
            Err(ResolveError::CollectionDenied)
        );
    }
}
