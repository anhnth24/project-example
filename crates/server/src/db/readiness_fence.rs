//! System-scoped restore/reconcile readiness fence.
//!
//! This repository intentionally does not use org-scoped transactions: the
//! backing table contains no tenant data and is read by readiness without an
//! org context.

use deadpool_postgres::Pool;

use crate::db::error::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessFenceState {
    Ready,
    Reconciling,
    Restoring,
}

impl ReadinessFenceState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Reconciling => "reconciling",
            Self::Restoring => "restoring",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ready" => Ok(Self::Ready),
            "reconciling" => Ok(Self::Reconciling),
            "restoring" => Ok(Self::Restoring),
            other => Err(format!("unknown readiness fence state: {other}")),
        }
    }
}

pub async fn current_state(pool: &Pool) -> Result<ReadinessFenceState, DbError> {
    let client = pool.get().await?;
    let row = client
        .query_opt("SELECT state FROM readiness_fence WHERE id = 1", &[])
        .await?
        .ok_or(DbError::NotFound)?;
    let state: String = row.get("state");
    ReadinessFenceState::parse(&state).map_err(DbError::Config)
}

pub async fn set_state(
    pool: &Pool,
    state: ReadinessFenceState,
    reason: Option<&str>,
) -> Result<(), DbError> {
    let client = pool.get().await?;
    let state = state.as_str();
    let reason = reason.map(str::trim).filter(|value| !value.is_empty());
    let updated = client
        .execute(
            "UPDATE readiness_fence
             SET state = $1, reason = $2, updated_at = clock_timestamp()
             WHERE id = 1",
            &[&state, &reason],
        )
        .await?;
    if updated == 1 {
        Ok(())
    } else {
        Err(DbError::NotFound)
    }
}

pub async fn set_reconciling(pool: &Pool, reason: Option<&str>) -> Result<(), DbError> {
    set_state(pool, ReadinessFenceState::Reconciling, reason).await
}

pub async fn set_ready(pool: &Pool, reason: Option<&str>) -> Result<(), DbError> {
    set_state(pool, ReadinessFenceState::Ready, reason).await
}

#[cfg(test)]
mod tests {
    use super::ReadinessFenceState;

    #[test]
    fn maps_wire_states() {
        for (wire, state) in [
            ("ready", ReadinessFenceState::Ready),
            ("reconciling", ReadinessFenceState::Reconciling),
            ("restoring", ReadinessFenceState::Restoring),
        ] {
            assert_eq!(ReadinessFenceState::parse(wire).unwrap(), state);
            assert_eq!(state.as_str(), wire);
        }
    }

    #[test]
    fn rejects_unknown_wire_state() {
        assert!(ReadinessFenceState::parse("stale").is_err());
    }
}
