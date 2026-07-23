//! Durable global restore/reconcile fences (P1B-R06 / O03).
//!
//! Fences are process/org-agnostic rows in `ops_fences`. Readiness fails closed
//! while any relevant fence is active. Backup sets the fence; restore clears
//! it only after a machine-verifiable attestation digest is recorded.

use deadpool_postgres::Pool;
use thiserror::Error;

pub const FENCE_RESTORE: &str = "restore";
pub const FENCE_RECONCILE: &str = "reconcile";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FenceError {
    #[error("database error")]
    Database,
    #[error("attestation required")]
    AttestationRequired,
    #[error("fence not active")]
    NotActive,
    #[error("ops fence active; mutations paused")]
    Active,
}

/// Stable API/error code when mutations are refused due to an ops fence.
pub const MUTATIONS_PAUSED_CODE: &str = "ops_fence_active";

impl From<crate::db::error::DbError> for FenceError {
    fn from(_: crate::db::error::DbError) -> Self {
        Self::Database
    }
}

/// True when restore or reconcile fence is active (SECURITY DEFINER aggregate).
pub async fn any_blocking_fence_active(pool: &Pool) -> Result<bool, FenceError> {
    let client = pool.get().await.map_err(|_| FenceError::Database)?;
    let active: bool = client
        .query_one("SELECT markhand_any_blocking_fence_active()", &[])
        .await
        .map_err(|_| FenceError::Database)?
        .get(0);
    Ok(active)
}

/// Fail-closed gate for app mutation routes during restore/reconcile fences.
pub async fn ensure_mutations_allowed(pool: &Pool) -> Result<(), FenceError> {
    if any_blocking_fence_active(pool).await? {
        return Err(FenceError::Active);
    }
    Ok(())
}

/// Maps fence check outcomes to a stable HTTP error code for mutation routes.
pub fn mutation_pause_code(error: &FenceError) -> &'static str {
    match error {
        FenceError::Active => MUTATIONS_PAUSED_CODE,
        FenceError::Database => "ops_fence_check_failed",
        FenceError::AttestationRequired | FenceError::NotActive => "ops_fence_check_failed",
    }
}

/// True when any org has an in-flight reconcile job (SECURITY DEFINER; all orgs).
pub async fn any_org_reconcile_running(pool: &Pool) -> Result<bool, FenceError> {
    let client = pool.get().await.map_err(|_| FenceError::Database)?;
    let running: bool = client
        .query_one("SELECT markhand_any_reconcile_running()", &[])
        .await
        .map_err(|_| FenceError::Database)?
        .get(0);
    Ok(running)
}

pub async fn set_fence(
    pool: &Pool,
    name: &str,
    reason: &str,
    set_by: Option<&str>,
) -> Result<(), FenceError> {
    let client = pool.get().await.map_err(|_| FenceError::Database)?;
    client
        .execute(
            "INSERT INTO ops_fences (name, reason, active, set_at, cleared_at, set_by)
             VALUES ($1, $2, true, now(), NULL, $3)
             ON CONFLICT (name) DO UPDATE
             SET reason = EXCLUDED.reason,
                 active = true,
                 set_at = now(),
                 cleared_at = NULL,
                 set_by = EXCLUDED.set_by,
                 attestation_sha256 = NULL",
            &[&name, &reason, &set_by],
        )
        .await
        .map_err(|_| FenceError::Database)?;
    Ok(())
}

/// Clears a fence only when a non-empty attestation digest is supplied.
pub async fn clear_fence_with_attestation(
    pool: &Pool,
    name: &str,
    attestation_sha256: &str,
) -> Result<(), FenceError> {
    if attestation_sha256.len() != 64
        || !attestation_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(FenceError::AttestationRequired);
    }
    let client = pool.get().await.map_err(|_| FenceError::Database)?;
    let updated = client
        .execute(
            "UPDATE ops_fences
             SET active = false,
                 cleared_at = now(),
                 attestation_sha256 = $2
             WHERE name = $1 AND active = true",
            &[&name, &attestation_sha256],
        )
        .await
        .map_err(|_| FenceError::Database)?;
    if updated == 0 {
        return Err(FenceError::NotActive);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_names_are_stable() {
        assert_eq!(FENCE_RESTORE, "restore");
        assert_eq!(FENCE_RECONCILE, "reconcile");
    }

    #[test]
    fn mutation_pause_code_is_stable_and_fail_closed() {
        assert_eq!(mutation_pause_code(&FenceError::Active), "ops_fence_active");
        assert_eq!(
            mutation_pause_code(&FenceError::Database),
            "ops_fence_check_failed"
        );
        assert_eq!(MUTATIONS_PAUSED_CODE, "ops_fence_active");
    }
}
