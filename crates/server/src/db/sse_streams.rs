//! Per-user/org/request closed SSE snapshot persistence (P1B-R05).
//!
//! Streams are committed atomically as contiguous metadata+token+terminal
//! sequences and are never durable while open. Auth scope is stored for
//! reconnect revalidation. Retention is bounded by max_events/max_bytes/TTL.

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

pub const DEFAULT_MAX_EVENTS: i32 = 4_200;
pub const DEFAULT_MAX_BYTES: i64 = 256 * 1024;
pub const DEFAULT_TTL_SECS: i64 = 15 * 60;
/// Always reserve one slot for the terminal close/error event.
pub const TERMINAL_EVENT_RESERVE: i32 = 1;
pub const DEFAULT_CLEANUP_LIMIT: i64 = 32;
/// Migration `payload_bytes` CHECK upper bound.
pub const MAX_EVENT_PAYLOAD_BYTES: i32 = 65_536;
/// Opportunistic cleanup waits this long after `expires_at` so the first GET
/// can still observe deterministic 410 before rows are removed.
pub const CLEANUP_GRACE_SECS: i64 = 60;

const REQUEST_COLUMNS: &str = "id, org_id, user_id, kind, status, close_reason, version_mode, \
     requires_history, collection_ids, cited_document_ids, cited_version_ids, \
     next_sequence, event_count, byte_count, max_events, max_bytes, expires_at, \
     created_at, closed_at";

const EVENT_COLUMNS: &str = "id, org_id, request_id, user_id, sequence_no, event_type, \
     envelope_version, data, payload_bytes, created_at";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseStreamKind {
    Ask,
}

impl SseStreamKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseStreamStatus {
    Closed,
    Error,
    Expired,
}

impl SseStreamStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Error => "error",
            Self::Expired => "expired",
        }
    }

    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "closed" => Ok(Self::Closed),
            "error" => Ok(Self::Error),
            "expired" => Ok(Self::Expired),
            _ => Err(DbError::Config("unknown sse stream status".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamAuthScope {
    pub version_mode: String,
    pub requires_history: bool,
    pub collection_ids: Vec<Uuid>,
    pub cited_document_ids: Vec<Uuid>,
    pub cited_version_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SseStreamRequest {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub status: SseStreamStatus,
    pub close_reason: String,
    pub auth_scope: StreamAuthScope,
    pub next_sequence: i64,
    pub event_count: i32,
    pub byte_count: i64,
    pub max_events: i32,
    pub max_bytes: i64,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub closed_at: DateTime<Utc>,
}

impl SseStreamRequest {
    pub fn high_water_sequence(&self) -> u64 {
        u64::try_from(self.next_sequence.saturating_sub(1)).unwrap_or(0)
    }
}

/// Result of an atomic DB-time load for closed-snapshot delivery.
#[derive(Debug, Clone, PartialEq)]
pub enum ClosedSnapshotLoad {
    /// Owned closed/error snapshot with `expires_at > clock_timestamp()`.
    Live(Box<SseStreamRequest>),
    /// Row exists for this owner but is past DB expiry (or already `expired`).
    Expired { request_id: Uuid },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SseStreamEvent {
    pub id: Uuid,
    pub org_id: Uuid,
    pub request_id: Uuid,
    pub user_id: Uuid,
    pub sequence_no: i64,
    pub event_type: String,
    pub envelope_version: i32,
    pub data: JsonValue,
    pub payload_bytes: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PlannedSseEvent {
    pub event_type: &'static str,
    pub data: JsonValue,
}

#[derive(Debug, Clone)]
pub struct NewClosedSnapshot {
    pub id: Uuid,
    pub kind: SseStreamKind,
    pub status: SseStreamStatus,
    pub close_reason: &'static str,
    pub auth_scope: StreamAuthScope,
    pub events: Vec<PlannedSseEvent>,
    pub max_events: i32,
    pub max_bytes: i64,
    pub ttl_secs: i64,
}

type PreparedEvent = (i64, &'static str, JsonValue, i32);

fn prepare_snapshot(input: &NewClosedSnapshot) -> Result<(i64, Vec<PreparedEvent>), DbError> {
    if matches!(input.status, SseStreamStatus::Expired) {
        return Err(DbError::Config("cannot persist expired snapshot".into()));
    }
    if input.max_events <= TERMINAL_EVENT_RESERVE
        || input.max_events > 8192
        || input.max_bytes <= 0
        || input.max_bytes > 1_048_576
        || input.ttl_secs <= 0
    {
        return Err(DbError::Config("invalid sse stream bounds".into()));
    }
    if input.auth_scope.collection_ids.is_empty() {
        return Err(DbError::Config(
            "sse auth scope collections required".into(),
        ));
    }
    if input.events.is_empty() {
        return Err(DbError::Config("sse snapshot requires events".into()));
    }
    let last = input.events.last().expect("non-empty");
    let expected_terminal = match input.status {
        SseStreamStatus::Closed => "close",
        SseStreamStatus::Error => "error",
        SseStreamStatus::Expired => unreachable!("expired rejected above"),
    };
    if last.event_type != expected_terminal {
        return Err(DbError::Config(
            "sse snapshot status and terminal event disagree".into(),
        ));
    }
    if input.status == SseStreamStatus::Closed && input.events.len() < 2 {
        return Err(DbError::Config(
            "closed sse snapshot requires metadata".into(),
        ));
    }
    if input.events.len() > 1 {
        if input.events[0].event_type != "metadata"
            || input.events[1..input.events.len() - 1]
                .iter()
                .any(|event| event.event_type != "token")
        {
            return Err(DbError::Config("invalid sse snapshot event order".into()));
        }
    }
    if input.events.len() as i32 > input.max_events {
        return Err(DbError::Config("sse snapshot exceeds max_events".into()));
    }

    let mut byte_count = 0i64;
    let mut prepared = Vec::with_capacity(input.events.len());
    for (idx, planned) in input.events.iter().enumerate() {
        let payload = serde_json::to_vec(&planned.data).unwrap_or_default();
        let payload_bytes = i32::try_from(payload.len()).unwrap_or(i32::MAX);
        if payload_bytes > MAX_EVENT_PAYLOAD_BYTES {
            return Err(DbError::Config(
                "sse event payload exceeds migration limit".into(),
            ));
        }
        byte_count = byte_count.saturating_add(i64::from(payload_bytes));
        if byte_count > input.max_bytes {
            return Err(DbError::Config("sse snapshot exceeds max_bytes".into()));
        }
        prepared.push((
            i64::try_from(idx + 1).unwrap_or(i64::MAX),
            planned.event_type,
            planned.data.clone(),
            payload_bytes,
        ));
    }
    Ok((byte_count, prepared))
}

/// Persist a complete closed snapshot in one transaction (create + events + closed).
///
/// On any failure the caller rolls back — no durable open/partial rows.
pub async fn persist_closed_snapshot(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewClosedSnapshot,
) -> Result<(SseStreamRequest, Vec<SseStreamEvent>), DbError> {
    let (byte_count, prepared) = prepare_snapshot(&input)?;

    lock_request(txn, ctx, input.id).await?;

    let event_count = i32::try_from(prepared.len()).unwrap_or(i32::MAX);
    let next_sequence = i64::from(event_count) + 1;
    let ttl = input.ttl_secs as f64;
    let collection_ids = input.auth_scope.collection_ids.clone();
    let cited_docs = input.auth_scope.cited_document_ids.clone();
    let cited_versions = input.auth_scope.cited_version_ids.clone();

    let request_row = txn
        .query_one(
            &format!(
                "INSERT INTO sse_stream_requests (
                    id, org_id, user_id, kind, status, close_reason,
                    version_mode, requires_history, collection_ids,
                    cited_document_ids, cited_version_ids,
                    next_sequence, event_count, byte_count,
                    max_events, max_bytes, expires_at, closed_at
                 ) VALUES (
                    $1, $2, $3, $4, $5, $6,
                    $7, $8, $9,
                    $10, $11,
                    $12, $13, $14,
                    $15, $16,
                    clock_timestamp() + make_interval(secs => $17::double precision),
                    clock_timestamp()
                 )
                 RETURNING {REQUEST_COLUMNS}"
            ),
            &[
                &input.id,
                &ctx.org_id(),
                &ctx.user_id(),
                &input.kind.as_str(),
                &input.status.as_str(),
                &input.close_reason,
                &input.auth_scope.version_mode,
                &input.auth_scope.requires_history,
                &collection_ids,
                &cited_docs,
                &cited_versions,
                &next_sequence,
                &event_count,
                &byte_count,
                &input.max_events,
                &input.max_bytes,
                &ttl,
            ],
        )
        .await?;

    let mut events = Vec::with_capacity(prepared.len());
    for (sequence_no, event_type, data, payload_bytes) in prepared {
        let row = txn
            .query_one(
                &format!(
                    "INSERT INTO sse_stream_events (
                        org_id, request_id, user_id, sequence_no, event_type,
                        envelope_version, data, payload_bytes
                     ) VALUES ($1, $2, $3, $4, $5, 1, $6, $7)
                     RETURNING {EVENT_COLUMNS}"
                ),
                &[
                    &ctx.org_id(),
                    &input.id,
                    &ctx.user_id(),
                    &sequence_no,
                    &event_type,
                    &data,
                    &payload_bytes,
                ],
            )
            .await?;
        events.push(map_event(&row)?);
    }

    Ok((map_request(&request_row)?, events))
}

/// Load owned stream. Missing / wrong user → NotFound (IDOR-safe).
pub async fn get_owned_request(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    request_id: Uuid,
) -> Result<SseStreamRequest, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {REQUEST_COLUMNS}
                 FROM sse_stream_requests
                 WHERE org_id = $1 AND id = $2 AND user_id = $3"
            ),
            &[&ctx.org_id(), &request_id, &ctx.user_id()],
        )
        .await?;
    row.map(|row| map_request(&row))
        .transpose()?
        .ok_or(DbError::NotFound)
}

/// Atomic DB-time load: live closed snapshot vs distinct expired-if-existed.
///
/// Live requires `status IN ('closed','error')` and `expires_at > clock_timestamp()`.
/// Does not delete rows — preserves deterministic 410 on first GET.
pub async fn load_owned_closed_request(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    request_id: Uuid,
) -> Result<ClosedSnapshotLoad, DbError> {
    let live = txn
        .query_opt(
            &format!(
                "SELECT {REQUEST_COLUMNS}
                 FROM sse_stream_requests
                 WHERE org_id = $1 AND id = $2 AND user_id = $3
                   AND status IN ('closed', 'error')
                   AND expires_at > clock_timestamp()"
            ),
            &[&ctx.org_id(), &request_id, &ctx.user_id()],
        )
        .await?;
    if let Some(row) = live {
        return Ok(ClosedSnapshotLoad::Live(Box::new(map_request(&row)?)));
    }

    let existed = txn
        .query_opt(
            "SELECT id FROM sse_stream_requests
             WHERE org_id = $1 AND id = $2 AND user_id = $3",
            &[&ctx.org_id(), &request_id, &ctx.user_id()],
        )
        .await?;
    if existed.is_some() {
        return Ok(ClosedSnapshotLoad::Expired { request_id });
    }
    Err(DbError::NotFound)
}

/// Events with `sequence_no > after_sequence` in ascending contiguous order.
pub async fn list_events_after(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    request_id: Uuid,
    after_sequence: u64,
) -> Result<Vec<SseStreamEvent>, DbError> {
    let _ = get_owned_request(txn, ctx, request_id).await?;
    let after = i64::try_from(after_sequence).map_err(|_| DbError::Config("sequence".into()))?;
    let rows = txn
        .query(
            &format!(
                "SELECT {EVENT_COLUMNS}
                 FROM sse_stream_events
                 WHERE org_id = $1 AND request_id = $2 AND user_id = $3
                   AND sequence_no > $4
                 ORDER BY sequence_no ASC"
            ),
            &[&ctx.org_id(), &request_id, &ctx.user_id(), &after],
        )
        .await?;
    rows.iter().map(map_event).collect()
}

/// Delete expired owned streams (cascade events). Returns deleted request count.
///
/// - Excludes `exclude_request_id` (active delivery / just-loaded stream).
/// - Deletes rows already marked `expired` (post-410), or past
///   `expires_at + CLEANUP_GRACE_SECS` (bounded grace tombstone).
/// - Never removes a just-expired live row before the first GET can return 410.
pub async fn cleanup_expired(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    limit: i64,
    exclude_request_id: Option<Uuid>,
) -> Result<u64, DbError> {
    let limit = limit.clamp(1, 256);
    let grace = CLEANUP_GRACE_SECS as f64;
    let deleted = txn
        .execute(
            "WITH doomed AS (
                 SELECT id FROM sse_stream_requests
                 WHERE org_id = $1
                   AND ($3::uuid IS NULL OR id <> $3)
                   AND (
                     status = 'expired'
                     OR expires_at <= clock_timestamp()
                         - make_interval(secs => $4::double precision)
                   )
                 ORDER BY expires_at ASC
                 LIMIT $2
                 FOR UPDATE SKIP LOCKED
             )
             DELETE FROM sse_stream_requests r
             USING doomed d
             WHERE r.org_id = $1 AND r.id = d.id",
            &[&ctx.org_id(), &limit, &exclude_request_id, &grace],
        )
        .await?;
    Ok(deleted)
}

/// Mark one owned stream expired then delete it (cascade). IDOR → NotFound.
pub async fn expire_and_delete(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    request_id: Uuid,
) -> Result<(), DbError> {
    lock_request(txn, ctx, request_id).await?;
    let updated = txn
        .execute(
            "UPDATE sse_stream_requests
             SET status = 'expired',
                 close_reason = 'expired',
                 closed_at = COALESCE(closed_at, clock_timestamp())
             WHERE org_id = $1 AND id = $2 AND user_id = $3",
            &[&ctx.org_id(), &request_id, &ctx.user_id()],
        )
        .await?;
    if updated == 0 {
        return Err(DbError::NotFound);
    }
    let deleted = txn
        .execute(
            "DELETE FROM sse_stream_requests
             WHERE org_id = $1 AND id = $2 AND user_id = $3",
            &[&ctx.org_id(), &request_id, &ctx.user_id()],
        )
        .await?;
    if deleted == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// Cited document pin row for reconnect revalidation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitedDocumentPin {
    pub id: Uuid,
    pub collection_id: Uuid,
    pub state: String,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Cited version pin row joined to its document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitedVersionPin {
    pub version_id: Uuid,
    pub document_id: Uuid,
    pub publication_state: String,
    pub collection_id: Uuid,
    pub document_state: String,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Load current cited document pins (caller applies ACL/state decisions).
pub async fn load_cited_document_pins(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    document_ids: &[Uuid],
) -> Result<Vec<CitedDocumentPin>, DbError> {
    if document_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT id, collection_id, state, deleted_at
             FROM documents
             WHERE org_id = $1 AND id = ANY($2::uuid[])",
            &[&ctx.org_id(), &document_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| CitedDocumentPin {
            id: row.get("id"),
            collection_id: row.get("collection_id"),
            state: row.get("state"),
            deleted_at: row.get("deleted_at"),
        })
        .collect())
}

/// Load current cited version pins joined to documents.
pub async fn load_cited_version_pins(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    version_ids: &[Uuid],
) -> Result<Vec<CitedVersionPin>, DbError> {
    if version_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = txn
        .query(
            "SELECT v.id AS version_id,
                    v.document_id,
                    v.publication_state,
                    d.collection_id,
                    d.state AS document_state,
                    d.deleted_at
             FROM document_versions v
             JOIN documents d
               ON d.org_id = v.org_id AND d.id = v.document_id
             WHERE v.org_id = $1 AND v.id = ANY($2::uuid[])",
            &[&ctx.org_id(), &version_ids],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| CitedVersionPin {
            version_id: row.get("version_id"),
            document_id: row.get("document_id"),
            publication_state: row.get("publication_state"),
            collection_id: row.get("collection_id"),
            document_state: row.get("document_state"),
            deleted_at: row.get("deleted_at"),
        })
        .collect())
}

async fn lock_request(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    request_id: Uuid,
) -> Result<(), DbError> {
    let key = format!("ssestream:{}:{}", ctx.org_id(), request_id);
    txn.execute("SELECT pg_advisory_xact_lock(hashtext($1))", &[&key])
        .await?;
    Ok(())
}

fn map_request(row: &Row) -> Result<SseStreamRequest, DbError> {
    let status: String = row.get("status");
    Ok(SseStreamRequest {
        id: row.get("id"),
        org_id: row.get("org_id"),
        user_id: row.get("user_id"),
        kind: row.get("kind"),
        status: SseStreamStatus::parse(&status)?,
        close_reason: row.get("close_reason"),
        auth_scope: StreamAuthScope {
            version_mode: row.get("version_mode"),
            requires_history: row.get("requires_history"),
            collection_ids: row.get("collection_ids"),
            cited_document_ids: row.get("cited_document_ids"),
            cited_version_ids: row.get("cited_version_ids"),
        },
        next_sequence: row.get("next_sequence"),
        event_count: row.get("event_count"),
        byte_count: row.get("byte_count"),
        max_events: row.get("max_events"),
        max_bytes: row.get("max_bytes"),
        expires_at: row.get("expires_at"),
        created_at: row.get("created_at"),
        closed_at: row.get("closed_at"),
    })
}

fn map_event(row: &Row) -> Result<SseStreamEvent, DbError> {
    Ok(SseStreamEvent {
        id: row.get("id"),
        org_id: row.get("org_id"),
        request_id: row.get("request_id"),
        user_id: row.get("user_id"),
        sequence_no: row.get("sequence_no"),
        event_type: row.get("event_type"),
        envelope_version: row.get("envelope_version"),
        data: row.get("data"),
        payload_bytes: row.get("payload_bytes"),
        created_at: row.get("created_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(status: SseStreamStatus, events: Vec<PlannedSseEvent>) -> NewClosedSnapshot {
        NewClosedSnapshot {
            id: Uuid::new_v4(),
            kind: SseStreamKind::Ask,
            status,
            close_reason: if status == SseStreamStatus::Closed {
                "completed"
            } else {
                "truncated"
            },
            auth_scope: StreamAuthScope {
                version_mode: "current".into(),
                requires_history: false,
                collection_ids: vec![Uuid::new_v4()],
                cited_document_ids: vec![],
                cited_version_ids: vec![],
            },
            events,
            max_events: DEFAULT_MAX_EVENTS,
            max_bytes: DEFAULT_MAX_BYTES,
            ttl_secs: DEFAULT_TTL_SECS,
        }
    }

    fn event(event_type: &'static str, data: JsonValue) -> PlannedSseEvent {
        PlannedSseEvent { event_type, data }
    }

    #[test]
    fn snapshot_preflight_accepts_canonical_closed_and_error_sequences() {
        let closed = snapshot(
            SseStreamStatus::Closed,
            vec![
                event("metadata", serde_json::json!({"grounded": true})),
                event("token", serde_json::json!({"text": "xin "})),
                event("token", serde_json::json!({"text": "chào"})),
                event("close", serde_json::json!({"reason": "completed"})),
            ],
        );
        let (bytes, prepared) = prepare_snapshot(&closed).unwrap();
        assert!(bytes > 0);
        assert_eq!(
            prepared
                .iter()
                .map(|row| (row.0, row.1))
                .collect::<Vec<_>>(),
            vec![(1, "metadata"), (2, "token"), (3, "token"), (4, "close")]
        );

        let error = snapshot(
            SseStreamStatus::Error,
            vec![event("error", serde_json::json!({"reason": "truncated"}))],
        );
        assert!(prepare_snapshot(&error).is_ok());
    }

    #[test]
    fn snapshot_preflight_rejects_status_order_scope_and_bound_violations() {
        let canonical = snapshot(
            SseStreamStatus::Closed,
            vec![
                event("metadata", serde_json::json!({})),
                event("close", serde_json::json!({"reason": "completed"})),
            ],
        );

        let mut expired = canonical.clone();
        expired.status = SseStreamStatus::Expired;
        assert!(prepare_snapshot(&expired).is_err());

        let mut mismatched = canonical.clone();
        mismatched.status = SseStreamStatus::Error;
        assert!(prepare_snapshot(&mismatched).is_err());

        let missing_metadata = snapshot(
            SseStreamStatus::Closed,
            vec![event("close", serde_json::json!({"reason": "completed"}))],
        );
        assert!(prepare_snapshot(&missing_metadata).is_err());

        let invalid_order = snapshot(
            SseStreamStatus::Closed,
            vec![
                event("token", serde_json::json!({"text": "early"})),
                event("close", serde_json::json!({"reason": "completed"})),
            ],
        );
        assert!(prepare_snapshot(&invalid_order).is_err());

        let early_terminal = snapshot(
            SseStreamStatus::Closed,
            vec![
                event("metadata", serde_json::json!({})),
                event("error", serde_json::json!({"reason": "early"})),
                event("close", serde_json::json!({"reason": "completed"})),
            ],
        );
        assert!(prepare_snapshot(&early_terminal).is_err());

        let mut no_scope = canonical.clone();
        no_scope.auth_scope.collection_ids.clear();
        assert!(prepare_snapshot(&no_scope).is_err());

        let mut too_many = canonical.clone();
        too_many.max_events = TERMINAL_EVENT_RESERVE + 1;
        too_many
            .events
            .insert(1, event("token", serde_json::json!({"text": "overflow"})));
        assert!(prepare_snapshot(&too_many).is_err());

        let oversized_payload = snapshot(
            SseStreamStatus::Error,
            vec![event(
                "error",
                serde_json::json!({"reason": "x".repeat(MAX_EVENT_PAYLOAD_BYTES as usize)}),
            )],
        );
        assert!(prepare_snapshot(&oversized_payload).is_err());

        let mut over_total = canonical;
        over_total.max_bytes = 1;
        assert!(prepare_snapshot(&over_total).is_err());
    }
}
