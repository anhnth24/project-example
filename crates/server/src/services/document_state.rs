//! Legal document lifecycle transitions over `documents.state`.
//!
//! # Transition table
//!
//! | From        | Allowed targets                         |
//! |-------------|-----------------------------------------|
//! | uploaded    | converting, failed                      |
//! | converting  | converted, failed                       |
//! | converted   | indexing, failed                        |
//! | indexing    | indexed, failed                         |
//! | indexed     | tombstoned, failed                      |
//! | failed      | converting _(only)_                     |
//! | tombstoned  | purged                                  |
//! | purged      | _(terminal)_                            |
//!
//! Happy path: `uploaded → converting → converted → indexing → indexed`.
//! Any active state may fail. Soft-delete is `indexed → tombstoned → purged`.
//!
//! ## Failed retry policy (no failure-stage column in F03 schema)
//!
//! `documents.state = 'failed'` does not record which stage failed, so a retry
//! cannot safely jump to `indexing`. Retries are restricted to
//! `failed → converting` (full pipeline restart). Restoring
//! `failed → indexing` requires a future failure-stage / provenance field.
//!
//! Transitions are applied under `SELECT … FOR UPDATE` plus a compare-and-set
//! `UPDATE … WHERE state = $expected` so concurrent attempts serialize: exactly
//! one commits.

use deadpool_postgres::Pool;
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::documents;
use crate::db::error::DbError;
use crate::db::models::{Document, DocumentState};
use crate::db::pool::with_org_txn;

/// Returns whether `from → to` is a legal lifecycle edge.
pub fn is_legal_transition(from: DocumentState, to: DocumentState) -> bool {
    use DocumentState::*;
    matches!(
        (from, to),
        (Uploaded, Converting)
            | (Uploaded, Failed)
            | (Converting, Converted)
            | (Converting, Failed)
            | (Converted, Indexing)
            | (Converted, Failed)
            | (Indexing, Indexed)
            | (Indexing, Failed)
            | (Indexed, Tombstoned)
            | (Indexed, Failed)
            // No failure-stage provenance in schema: only full convert retry.
            | (Failed, Converting)
            | (Tombstoned, Purged)
    )
}

/// Applies `to` if the locked row's current state matches `expected_from`.
///
/// Acquires `SELECT … FOR UPDATE`, validates the legal transition table, then
/// compare-and-sets the state. On illegal or stale transitions this returns
/// [`DbError::IllegalTransition`] / [`DbError::StaleState`] without mutating.
pub async fn apply_transition(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_id: Uuid,
    expected_from: DocumentState,
    to: DocumentState,
) -> Result<Document, DbError> {
    let current = documents::get_by_id_for_update(txn, ctx, document_id).await?;
    if current.state != expected_from {
        return Err(DbError::StaleState {
            expected: expected_from.to_string(),
            observed: current.state.to_string(),
        });
    }
    if !is_legal_transition(current.state, to) {
        return Err(DbError::IllegalTransition {
            from: current.state.to_string(),
            to: to.to_string(),
        });
    }
    documents::cas_state(txn, ctx, document_id, expected_from, to).await
}

/// Convenience: open an org transaction, lock, and transition.
///
/// Keeps the transaction short — no network/converter/LLM work inside.
pub async fn transition(
    pool: &Pool,
    ctx: &OrgContext,
    document_id: Uuid,
    expected_from: DocumentState,
    to: DocumentState,
) -> Result<Document, DbError> {
    let owned = ctx.clone();
    with_org_txn(pool, ctx, move |txn| {
        Box::pin(async move { apply_transition(txn, &owned, document_id, expected_from, to).await })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_and_terminal_edges() {
        assert!(is_legal_transition(
            DocumentState::Uploaded,
            DocumentState::Converting
        ));
        assert!(is_legal_transition(
            DocumentState::Converting,
            DocumentState::Converted
        ));
        assert!(is_legal_transition(
            DocumentState::Converted,
            DocumentState::Indexing
        ));
        assert!(is_legal_transition(
            DocumentState::Indexing,
            DocumentState::Indexed
        ));
        assert!(is_legal_transition(
            DocumentState::Indexed,
            DocumentState::Tombstoned
        ));
        assert!(is_legal_transition(
            DocumentState::Tombstoned,
            DocumentState::Purged
        ));
        assert!(!is_legal_transition(
            DocumentState::Purged,
            DocumentState::Uploaded
        ));
        assert!(!is_legal_transition(
            DocumentState::Uploaded,
            DocumentState::Indexed
        ));
        assert!(is_legal_transition(
            DocumentState::Failed,
            DocumentState::Converting
        ));
        // Without failure-stage provenance, indexing retry from failed is illegal.
        assert!(!is_legal_transition(
            DocumentState::Failed,
            DocumentState::Indexing
        ));
        assert!(!is_legal_transition(
            DocumentState::Uploaded,
            DocumentState::Indexing
        ));
    }
}
