//! Persisted HTTP Idempotency-Key claim/finalize for upload/reindex scopes.
//!
//! Flow: claim `in_progress` → perform work → finalize `completed` (exact replay).
//! Concurrent losers `SELECT FOR UPDATE` and either replay the completed response
//! or observe in-progress / hash conflict. Expired in-progress rows may be taken
//! over for recovery after a crashed worker.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use tokio_postgres::Transaction;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

/// Default lease for an in-progress claim before takeover is allowed.
pub const DEFAULT_IN_PROGRESS_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyScope {
    Upload,
    Reindex,
}

impl IdempotencyScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Reindex => "reindex",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredIdempotencyResponse {
    pub request_hash: String,
    pub response_status: i32,
    pub response_body: JsonValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IdempotencyClaim {
    /// Caller owns the key and must finalize or abandon.
    Proceed,
    /// Prior identical request finished; return this response verbatim.
    Replay(StoredIdempotencyResponse),
}

#[derive(Debug, Clone, PartialEq)]
struct StoredRow {
    state: String,
    request_hash: String,
    response_status: Option<i32>,
    response_body: Option<JsonValue>,
    expires_at: DateTime<Utc>,
}

/// Claim an idempotency key or return the completed original response.
///
/// Must run inside the same transaction as reindex enqueue/finalize so losers
/// block on `FOR UPDATE` and read the exact committed original.
pub async fn claim_or_replay(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    scope: IdempotencyScope,
    idempotency_key: &str,
    request_hash: &str,
    ttl: Duration,
) -> Result<IdempotencyClaim, DbError> {
    let scope_str = scope.as_str();
    let ttl_secs = i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX);
    let inserted = txn
        .query_opt(
            "INSERT INTO api_idempotency_keys (
                org_id, user_id, scope, idempotency_key, state, request_hash,
                expires_at
             ) VALUES (
                $1, $2, $3, $4, 'in_progress', $5,
                clock_timestamp() + make_interval(secs => $6::double precision)
             )
             ON CONFLICT (org_id, user_id, scope, idempotency_key) DO NOTHING
             RETURNING id",
            &[
                &ctx.org_id(),
                &ctx.user_id(),
                &scope_str,
                &idempotency_key,
                &request_hash,
                &(ttl_secs as f64),
            ],
        )
        .await?;
    if inserted.is_some() {
        return Ok(IdempotencyClaim::Proceed);
    }

    let row = txn
        .query_one(
            "SELECT state, request_hash, response_status, response_body, expires_at
             FROM api_idempotency_keys
             WHERE org_id = $1
               AND user_id = $2
               AND scope = $3
               AND idempotency_key = $4
             FOR UPDATE",
            &[&ctx.org_id(), &ctx.user_id(), &scope_str, &idempotency_key],
        )
        .await?;
    let stored = StoredRow {
        state: row.get("state"),
        request_hash: row.get("request_hash"),
        response_status: row.get("response_status"),
        response_body: row.get("response_body"),
        expires_at: row.get("expires_at"),
    };
    if stored.request_hash != request_hash {
        return Err(DbError::Config("idempotency_key_conflict".into()));
    }
    match stored.state.as_str() {
        "completed" => {
            let status = stored
                .response_status
                .ok_or_else(|| DbError::Config("idempotency_corrupt".into()))?;
            let body = stored
                .response_body
                .ok_or_else(|| DbError::Config("idempotency_corrupt".into()))?;
            Ok(IdempotencyClaim::Replay(StoredIdempotencyResponse {
                request_hash: stored.request_hash,
                response_status: status,
                response_body: body,
            }))
        }
        "in_progress" => {
            let now: DateTime<Utc> = txn.query_one("SELECT clock_timestamp()", &[]).await?.get(0);
            if stored.expires_at > now {
                return Err(DbError::Config("idempotency_in_progress".into()));
            }
            // Expired claim: take over for recovery with the same hash.
            let updated = txn
                .execute(
                    "UPDATE api_idempotency_keys
                     SET state = 'in_progress',
                         request_hash = $5,
                         response_status = NULL,
                         response_body = NULL,
                         updated_at = clock_timestamp(),
                         expires_at = clock_timestamp()
                             + make_interval(secs => $6::double precision)
                     WHERE org_id = $1
                       AND user_id = $2
                       AND scope = $3
                       AND idempotency_key = $4
                       AND state = 'in_progress'",
                    &[
                        &ctx.org_id(),
                        &ctx.user_id(),
                        &scope_str,
                        &idempotency_key,
                        &request_hash,
                        &(ttl_secs as f64),
                    ],
                )
                .await?;
            if updated != 1 {
                return Err(DbError::Config("idempotency_in_progress".into()));
            }
            Ok(IdempotencyClaim::Proceed)
        }
        _ => Err(DbError::Config("idempotency_corrupt".into())),
    }
}

/// Finalize a claimed key with the exact response that losers must replay.
pub async fn finalize(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    scope: IdempotencyScope,
    idempotency_key: &str,
    request_hash: &str,
    response_status: i32,
    response_body: &JsonValue,
) -> Result<(), DbError> {
    let scope_str = scope.as_str();
    let updated = txn
        .execute(
            "UPDATE api_idempotency_keys
             SET state = 'completed',
                 response_status = $5,
                 response_body = $6,
                 updated_at = clock_timestamp(),
                 expires_at = clock_timestamp() + interval '30 days'
             WHERE org_id = $1
               AND user_id = $2
               AND scope = $3
               AND idempotency_key = $4
               AND request_hash = $7
               AND state = 'in_progress'",
            &[
                &ctx.org_id(),
                &ctx.user_id(),
                &scope_str,
                &idempotency_key,
                &response_status,
                &response_body,
                &request_hash,
            ],
        )
        .await?;
    if updated != 1 {
        return Err(DbError::Config("idempotency_finalize_failed".into()));
    }
    Ok(())
}

/// Drop an in-progress claim so a retry can proceed (no completed response).
pub async fn abandon(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    scope: IdempotencyScope,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<(), DbError> {
    let scope_str = scope.as_str();
    txn.execute(
        "DELETE FROM api_idempotency_keys
         WHERE org_id = $1
           AND user_id = $2
           AND scope = $3
           AND idempotency_key = $4
           AND request_hash = $5
           AND state = 'in_progress'",
        &[
            &ctx.org_id(),
            &ctx.user_id(),
            &scope_str,
            &idempotency_key,
            &request_hash,
        ],
    )
    .await?;
    Ok(())
}

pub fn hash_request_parts(parts: &[&[u8]]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0xff]);
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn hash_is_stable_for_same_parts() {
        let a = hash_request_parts(&[b"reindex", Uuid::nil().as_bytes()]);
        let b = hash_request_parts(&[b"reindex", Uuid::nil().as_bytes()]);
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn upload_hash_includes_filename_and_content_type() {
        let base = hash_request_parts(&[b"upload", b"abc", b"10", b"", b""]);
        let named = hash_request_parts(&[b"upload", b"abc", b"10", b"a.pdf", b"application/pdf"]);
        assert_ne!(base, named);
        let other_name =
            hash_request_parts(&[b"upload", b"abc", b"10", b"b.pdf", b"application/pdf"]);
        assert_ne!(named, other_name);
    }
}
