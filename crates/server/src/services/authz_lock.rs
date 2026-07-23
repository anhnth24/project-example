//! Shared principal authorization serialization for upload saga and ACL mutations.
//!
//! Lock order (must be respected by all callers):
//! 1. Refresh-token family advisory (`auth::session::lock_refresh_family`) when the
//!    path is session-scoped (ask SSE append/send, logout/refresh). Compatible with
//!    auth: logout/refresh take family only; revoke-all takes user lock then families.
//! 2. `pg_advisory_xact_lock(principal_authz_key(org, user))`
//! 3. `org_memberships` / `users` / `collections` / `role_permissions` /
//!    `collection_user_access` row locks as needed
//! 4. upload_operations row (`FOR UPDATE`) / ask stream session advisory
//! 5. quota admission locks

use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::db::error::DbError;

/// Canonical advisory-lock key shared by saga registration and permission/ACL writers.
pub fn principal_authz_lock_key(org_id: Uuid, user_id: Uuid) -> String {
    format!("authz-principal:{org_id}:{user_id}")
}

/// Acquire the shared principal authz advisory lock inside the current txn.
pub async fn lock_principal_authz(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<(), DbError> {
    let key = principal_authz_lock_key(org_id, user_id);
    txn.query_one("SELECT pg_advisory_xact_lock(hashtext($1))", &[&key])
        .await
        .map_err(DbError::from)?;
    Ok(())
}

/// Helper for ACL/permission mutation paths: take the shared principal lock first.
pub async fn with_principal_authz_lock<T, F, Fut>(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
    f: F,
) -> Result<T, DbError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, DbError>>,
{
    lock_principal_authz(txn, org_id, user_id).await?;
    f().await
}
