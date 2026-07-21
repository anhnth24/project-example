//! Tenant-scoped single-use download capability repository (P1B-R02).
//!
//! Issuance and consume expiry are evaluated with PostgreSQL `clock_timestamp()`
//! so app-side wall clocks before round-trips cannot skew single-use semantics.

use chrono::{DateTime, Utc};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

/// Download purpose bound into the capability (never inferred from client paths).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPurpose {
    Original,
    Markdown,
}

impl DownloadPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Markdown => "markdown",
        }
    }

    pub fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "original" => Ok(Self::Original),
            "markdown" => Ok(Self::Markdown),
            _ => Err(DbError::Config("unknown download purpose".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadCapabilityRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub purpose: DownloadPurpose,
    pub content_sha256: String,
    pub content_type: String,
    pub byte_size: i64,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewDownloadCapability<'a> {
    pub id: Uuid,
    pub document_id: Uuid,
    pub version_id: Uuid,
    pub purpose: DownloadPurpose,
    pub content_sha256: &'a str,
    pub content_type: &'a str,
    pub byte_size: i64,
    /// TTL seconds; `expires_at` / `created_at` are set from DB `clock_timestamp()`.
    pub ttl_secs: i64,
}

/// Outcome of an atomic consume attempt classified with the DB clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsumeOutcome {
    Consumed(DownloadCapabilityRow),
    Expired,
    Replay,
    NotFound,
}

/// Outcome of auth-gated consume. Wrong-user probes never surface Expired/Replay (IDOR).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizedConsumeOutcome {
    Consumed(DownloadCapabilityRow),
    Expired,
    Replay,
    PermissionDenied,
    NotFound,
}

/// Non-mutating liveness probe (DB clock). May race; consume is authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityLiveness {
    Open,
    Expired,
    Replay,
    NotFound,
}

/// Shared ACL / membership / document predicates for download redeem consume.
/// Mirrors `load_authorized_version_for_read` so revoke/delete cannot race past consume.
const AUTHORIZED_DOWNLOAD_EXISTS_SQL: &str = "
    EXISTS (
      SELECT 1
      FROM documents d
      JOIN document_versions dv
        ON dv.org_id = d.org_id
       AND dv.document_id = d.id
       AND dv.id = dc.version_id
      JOIN collections acl_c
        ON acl_c.org_id = d.org_id AND acl_c.id = d.collection_id
      JOIN org_memberships acl_m
        ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = $3
      JOIN users acl_u ON acl_u.id = acl_m.user_id
      JOIN roles acl_r
        ON acl_r.org_id = acl_m.org_id AND acl_r.code = acl_m.role
      JOIN role_permissions acl_rp
        ON acl_rp.org_id = acl_r.org_id AND acl_rp.role_id = acl_r.id
      JOIN permissions acl_p ON acl_p.id = acl_rp.permission_id
      WHERE d.org_id = dc.org_id
        AND d.id = dc.document_id
        AND d.deleted_at IS NULL
        AND d.state = 'indexed'
        AND dv.publication_state = 'published'
        AND acl_c.deleted_at IS NULL
        AND acl_u.disabled_at IS NULL
        AND acl_p.code = CASE WHEN dv.is_current THEN 'qa.query' ELSE 'qa.history' END
        AND EXISTS (
          SELECT 1
          FROM role_permissions query_rp
          JOIN permissions query_p ON query_p.id = query_rp.permission_id
          WHERE query_rp.org_id = acl_r.org_id
            AND query_rp.role_id = acl_r.id
            AND query_p.code = 'qa.query'
        )
        AND (
          acl_c.visibility = 'org'
          OR acl_c.owner_user_id = $3
          OR EXISTS (
            SELECT 1 FROM collection_user_access cua
            WHERE cua.org_id = acl_c.org_id
              AND cua.collection_id = acl_c.id
              AND cua.user_id = $3
          )
        )
    )";

pub async fn insert(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewDownloadCapability<'_>,
) -> Result<DownloadCapabilityRow, DbError> {
    if input.ttl_secs <= 0 {
        return Err(DbError::Config("invalid capability ttl".into()));
    }
    let purpose = input.purpose.as_str();
    let row = txn
        .query_one(
            "INSERT INTO download_capabilities (
                id, org_id, user_id, document_id, version_id, purpose,
                content_sha256, content_type, byte_size, expires_at, created_at
             )
             SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9,
                    clock_now + ($10::bigint * interval '1 second'),
                    clock_now
             FROM (SELECT clock_timestamp() AS clock_now) AS clock
             RETURNING id, org_id, user_id, document_id, version_id, purpose,
                       content_sha256, content_type, byte_size, expires_at,
                       consumed_at, created_at",
            &[
                &input.id,
                &ctx.org_id(),
                &ctx.user_id(),
                &input.document_id,
                &input.version_id,
                &purpose,
                &input.content_sha256,
                &input.content_type,
                &input.byte_size,
                &input.ttl_secs,
            ],
        )
        .await?;
    map_row(&row)
}

/// Atomically consumes an open, unexpired capability using DB `clock_timestamp()`.
pub async fn consume_if_open(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    capability_id: Uuid,
) -> Result<Option<DownloadCapabilityRow>, DbError> {
    let row = txn
        .query_opt(
            "UPDATE download_capabilities
             SET consumed_at = clock_timestamp()
             WHERE org_id = $1
               AND id = $2
               AND user_id = $3
               AND consumed_at IS NULL
               AND expires_at > clock_timestamp()
             RETURNING id, org_id, user_id, document_id, version_id, purpose,
                       content_sha256, content_type, byte_size, expires_at,
                       consumed_at, created_at",
            &[&ctx.org_id(), &capability_id, &ctx.user_id()],
        )
        .await?;
    row.map(|row| map_row(&row)).transpose()
}

/// Classify open/expired/replay using DB `clock_timestamp()` without consuming.
pub async fn classify_liveness(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    capability_id: Uuid,
) -> Result<CapabilityLiveness, DbError> {
    let row = txn
        .query_opt(
            "SELECT consumed_at IS NOT NULL AS consumed,
                    expires_at <= clock_timestamp() AS expired
             FROM download_capabilities
             WHERE org_id = $1 AND id = $2 AND user_id = $3",
            &[&ctx.org_id(), &capability_id, &ctx.user_id()],
        )
        .await?;
    let Some(row) = row else {
        return Ok(CapabilityLiveness::NotFound);
    };
    let consumed: bool = row.get(0);
    let expired: bool = row.get(1);
    if consumed {
        Ok(CapabilityLiveness::Replay)
    } else if expired {
        Ok(CapabilityLiveness::Expired)
    } else {
        Ok(CapabilityLiveness::Open)
    }
}

/// Consume when open; otherwise classify expired vs replay with the DB clock in
/// the same transaction (no app-side `Utc::now()`).
pub async fn consume_or_classify(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    capability_id: Uuid,
) -> Result<ConsumeOutcome, DbError> {
    if let Some(row) = consume_if_open(txn, ctx, capability_id).await? {
        return Ok(ConsumeOutcome::Consumed(row));
    }
    let row = txn
        .query_opt(
            "SELECT consumed_at IS NOT NULL AS consumed,
                    expires_at <= clock_timestamp() AS expired
             FROM download_capabilities
             WHERE org_id = $1 AND id = $2 AND user_id = $3",
            &[&ctx.org_id(), &capability_id, &ctx.user_id()],
        )
        .await?;
    let Some(row) = row else {
        return Ok(ConsumeOutcome::NotFound);
    };
    let consumed: bool = row.get(0);
    let expired: bool = row.get(1);
    if consumed {
        Ok(ConsumeOutcome::Replay)
    } else if expired {
        Ok(ConsumeOutcome::Expired)
    } else {
        // Lost the race to a concurrent consumer that has not yet become visible
        // as consumed under READ COMMITTED, or a transient predicate miss —
        // single-use semantics treat this as replay.
        Ok(ConsumeOutcome::Replay)
    }
}

/// Atomically consume only when live auth + document/version ACL still hold.
///
/// Holds `FOR UPDATE` on the capability row, then a single conditional
/// `UPDATE ... WHERE EXISTS (auth predicates)`. Revoke/delete concurrent with
/// this statement cannot yield a consumed row. Auth failure leaves the token
/// open (retryable). Wrong-user probes return [`AuthorizedConsumeOutcome::PermissionDenied`]
/// rather than Expired/Replay (no IDOR oracle).
pub async fn consume_authorized_or_classify(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    capability_id: Uuid,
    expected_document_id: Uuid,
    expected_version_id: Uuid,
    expected_purpose: DownloadPurpose,
    expected_content_sha256: &str,
    expected_content_type: &str,
    expected_byte_size: i64,
) -> Result<AuthorizedConsumeOutcome, DbError> {
    let purpose = expected_purpose.as_str();
    // Lock the capability row so classification and conditional consume see one snapshot.
    let locked = txn
        .query_opt(
            "SELECT id, org_id, user_id, document_id, version_id, purpose,
                    content_sha256, content_type, byte_size, expires_at,
                    consumed_at, created_at,
                    consumed_at IS NOT NULL AS was_consumed,
                    expires_at <= clock_timestamp() AS is_expired
             FROM download_capabilities
             WHERE org_id = $1 AND id = $2
             FOR UPDATE",
            &[&ctx.org_id(), &capability_id],
        )
        .await?;
    let Some(locked) = locked else {
        return Ok(AuthorizedConsumeOutcome::NotFound);
    };
    let owner: Uuid = locked.get("user_id");
    if owner != ctx.user_id() {
        // Do not reveal expired/replay to a different principal.
        return Ok(AuthorizedConsumeOutcome::PermissionDenied);
    }
    let was_consumed: bool = locked.get("was_consumed");
    if was_consumed {
        return Ok(AuthorizedConsumeOutcome::Replay);
    }
    let is_expired: bool = locked.get("is_expired");
    if is_expired {
        return Ok(AuthorizedConsumeOutcome::Expired);
    }

    let update_sql = format!(
        "UPDATE download_capabilities dc
         SET consumed_at = clock_timestamp()
         WHERE dc.org_id = $1
           AND dc.id = $2
           AND dc.user_id = $3
           AND dc.document_id = $4
           AND dc.version_id = $5
           AND dc.purpose = $6
           AND dc.content_sha256 = $7
           AND dc.content_type = $8
           AND dc.byte_size = $9
           AND dc.consumed_at IS NULL
           AND dc.expires_at > clock_timestamp()
           AND {AUTHORIZED_DOWNLOAD_EXISTS_SQL}
         RETURNING id, org_id, user_id, document_id, version_id, purpose,
                   content_sha256, content_type, byte_size, expires_at,
                   consumed_at, created_at"
    );
    let row = txn
        .query_opt(
            &update_sql,
            &[
                &ctx.org_id(),
                &capability_id,
                &ctx.user_id(),
                &expected_document_id,
                &expected_version_id,
                &purpose,
                &expected_content_sha256,
                &expected_content_type,
                &expected_byte_size,
            ],
        )
        .await?;
    if let Some(row) = row {
        return Ok(AuthorizedConsumeOutcome::Consumed(map_row(&row)?));
    }

    // Still locked: open + unexpired + owner, but UPDATE missed → auth/bindings failed.
    // Token remains unconsumed (retryable once access is restored).
    let auth_sql = format!(
        "SELECT {AUTHORIZED_DOWNLOAD_EXISTS_SQL} AS authorized
         FROM download_capabilities dc
         WHERE dc.org_id = $1 AND dc.id = $2 AND dc.user_id = $3"
    );
    let auth_row = txn
        .query_one(&auth_sql, &[&ctx.org_id(), &capability_id, &ctx.user_id()])
        .await?;
    let authorized: bool = auth_row.get(0);
    if !authorized {
        return Ok(AuthorizedConsumeOutcome::PermissionDenied);
    }
    // Bindings drifted or lost a concurrent consume race under the lock (should be rare).
    let again = txn
        .query_one(
            "SELECT consumed_at IS NOT NULL, expires_at <= clock_timestamp()
             FROM download_capabilities
             WHERE org_id = $1 AND id = $2
             FOR UPDATE",
            &[&ctx.org_id(), &capability_id],
        )
        .await?;
    let consumed_now: bool = again.get(0);
    let expired_now: bool = again.get(1);
    if consumed_now {
        Ok(AuthorizedConsumeOutcome::Replay)
    } else if expired_now {
        Ok(AuthorizedConsumeOutcome::Expired)
    } else {
        // Binding mismatch vs expected HMAC fields — fail closed without burning semantics leak.
        Ok(AuthorizedConsumeOutcome::PermissionDenied)
    }
}

pub async fn get_by_id(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    capability_id: Uuid,
) -> Result<Option<DownloadCapabilityRow>, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, user_id, document_id, version_id, purpose,
                    content_sha256, content_type, byte_size, expires_at,
                    consumed_at, created_at
             FROM download_capabilities
             WHERE org_id = $1 AND id = $2",
            &[&ctx.org_id(), &capability_id],
        )
        .await?;
    row.map(|row| map_row(&row)).transpose()
}

fn map_row(row: &Row) -> Result<DownloadCapabilityRow, DbError> {
    let purpose = DownloadPurpose::parse(row.get("purpose"))?;
    Ok(DownloadCapabilityRow {
        id: row.get("id"),
        org_id: row.get("org_id"),
        user_id: row.get("user_id"),
        document_id: row.get("document_id"),
        version_id: row.get("version_id"),
        purpose,
        content_sha256: row.get("content_sha256"),
        content_type: row.get("content_type"),
        byte_size: row.get("byte_size"),
        expires_at: row.get("expires_at"),
        consumed_at: row.get("consumed_at"),
        created_at: row.get("created_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purpose_round_trip() {
        assert_eq!(
            DownloadPurpose::parse("original").unwrap(),
            DownloadPurpose::Original
        );
        assert_eq!(
            DownloadPurpose::parse("markdown").unwrap(),
            DownloadPurpose::Markdown
        );
        assert!(DownloadPurpose::parse("export").is_err());
    }
}
