//! Cross-process authorization epochs for Q&A stream fencing (P1B-R03 / R05).
//!
//! Membership/ACL/document mutations bump epochs. Streams capture an epoch at
//! initial full auth and refuse delivery after a bump. Revoke paths wait for
//! in-process send permits to drain before commit/ack (see `qa::authz_fence`).

use chrono::{DateTime, Utc};
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::db::error::DbError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthzEpochSnapshot {
    pub user_epoch: i64,
    pub document_epochs_sum: i64,
    pub observed_at: DateTime<Utc>,
}

impl AuthzEpochSnapshot {
    /// Stable 64-bit composite for in-process fence capture/compare.
    pub fn composite(self) -> u64 {
        (self.user_epoch as u64)
            .wrapping_mul(1_000_003)
            .wrapping_add(self.document_epochs_sum as u64)
    }
}

/// Read user + cited-document epochs (missing rows ⇒ epoch 1).
pub async fn read_epoch_snapshot(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
) -> Result<AuthzEpochSnapshot, DbError> {
    read_epoch_snapshot_client(txn, org_id, user_id, document_ids).await
}

/// Same as [`read_epoch_snapshot`] on any [`tokio_postgres::GenericClient`].
pub async fn read_epoch_snapshot_on_client<C>(
    client: &C,
    org_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
) -> Result<AuthzEpochSnapshot, DbError>
where
    C: tokio_postgres::GenericClient + Sync,
{
    read_epoch_snapshot_client(client, org_id, user_id, document_ids).await
}

async fn read_epoch_snapshot_client<C>(
    client: &C,
    org_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
) -> Result<AuthzEpochSnapshot, DbError>
where
    C: tokio_postgres::GenericClient + Sync,
{
    let user_row = client
        .query_opt(
            "SELECT epoch FROM authz_epochs WHERE org_id = $1 AND user_id = $2",
            &[&org_id, &user_id],
        )
        .await?;
    let user_epoch: i64 = user_row.map(|r| r.get(0)).unwrap_or(1);
    let document_epochs_sum: i64 = if document_ids.is_empty() {
        0
    } else {
        let row = client
            .query_one(
                "SELECT COALESCE(SUM(epoch), 0)::bigint
                 FROM document_authz_epochs
                 WHERE org_id = $1 AND document_id = ANY($2)",
                &[&org_id, &document_ids],
            )
            .await?;
        let present: i64 = row.get(0);
        let missing = (document_ids.len() as i64).saturating_sub(
            client
                .query_one(
                    "SELECT COUNT(*)::bigint FROM document_authz_epochs
                     WHERE org_id = $1 AND document_id = ANY($2)",
                    &[&org_id, &document_ids],
                )
                .await?
                .get::<_, i64>(0),
        );
        present.saturating_add(missing)
    };
    Ok(AuthzEpochSnapshot {
        user_epoch,
        document_epochs_sum,
        observed_at: Utc::now(),
    })
}

/// Bump membership/ACL epoch for one user (creates row if absent).
pub async fn bump_user_epoch(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "INSERT INTO authz_epochs (org_id, user_id, epoch, updated_at)
             VALUES ($1, $2, 2, now())
             ON CONFLICT (org_id, user_id) DO UPDATE
               SET epoch = authz_epochs.epoch + 1,
                   updated_at = now()
             RETURNING epoch",
            &[&org_id, &user_id],
        )
        .await?;
    Ok(row.get(0))
}

/// Bump document epoch (tombstone/ACL mutation). Creates row at 2 if absent.
pub async fn bump_document_epoch(
    txn: &Transaction<'_>,
    org_id: Uuid,
    document_id: Uuid,
) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "INSERT INTO document_authz_epochs (org_id, document_id, epoch, updated_at)
             VALUES ($1, $2, 2, now())
             ON CONFLICT (org_id, document_id) DO UPDATE
               SET epoch = document_authz_epochs.epoch + 1,
                   updated_at = now()
             RETURNING epoch",
            &[&org_id, &document_id],
        )
        .await?;
    Ok(row.get(0))
}

/// Ensure epoch rows exist at 1 for stream capture (idempotent).
pub async fn ensure_user_epoch(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<i64, DbError> {
    let row = txn
        .query_one(
            "INSERT INTO authz_epochs (org_id, user_id, epoch, updated_at)
             VALUES ($1, $2, 1, now())
             ON CONFLICT (org_id, user_id) DO UPDATE
               SET updated_at = authz_epochs.updated_at
             RETURNING epoch",
            &[&org_id, &user_id],
        )
        .await?;
    Ok(row.get(0))
}
