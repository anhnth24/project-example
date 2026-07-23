//! Durable ask SSE sessions + monotonic event append (P1B-R05).
//!
//! Append/close paths take the refresh-family advisory lock (shared with auth
//! logout/refresh), then principal authz + FOR SHARE cited documents, so
//! logout/suspend/delete writers cannot race a post-revoke content write.

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value as JsonValue;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{resolve_org_context_on_txn, ResolveError};
use crate::auth::session::lock_refresh_family;
use crate::db::error::DbError;
use crate::db::models::DocumentState;
use crate::services::authz_lock;

pub const DEFAULT_MAX_EVENTS: i32 = 4_200;
pub const DEFAULT_MAX_BYTES: i64 = 256 * 1024;
pub const DEFAULT_TTL_SECS: i64 = 15 * 60;
pub const MAX_EVENT_PAYLOAD_BYTES: i32 = 65_536;
pub const PRODUCER_LEASE_SECS: i64 = 30;
pub const TERMINAL_EVENT_TYPE: &str = "stream.closed";

static PURGED_SESSIONS: AtomicU64 = AtomicU64::new(0);
static PURGED_EVENTS: AtomicU64 = AtomicU64::new(0);
static PRODUCER_RECOVERED: AtomicU64 = AtomicU64::new(0);

pub fn purged_sessions_total() -> u64 {
    PURGED_SESSIONS.load(Ordering::Relaxed)
}

pub fn purged_events_total() -> u64 {
    PURGED_EVENTS.load(Ordering::Relaxed)
}

pub fn producer_recovered_total() -> u64 {
    PRODUCER_RECOVERED.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskStreamStatus {
    Open,
    Closed,
    Error,
}

impl AskStreamStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Error => "error",
        }
    }

    fn parse(value: &str) -> Result<Self, DbError> {
        match value {
            "open" => Ok(Self::Open),
            "closed" => Ok(Self::Closed),
            "error" => Ok(Self::Error),
            _ => Err(DbError::Config("unknown ask stream status".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AskStreamSession {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub status: AskStreamStatus,
    pub close_reason: Option<String>,
    pub version_mode: String,
    pub collection_ids: Vec<Uuid>,
    pub cited_document_ids: Vec<Uuid>,
    pub cited_version_ids: Vec<Uuid>,
    pub pinned_snapshot: JsonValue,
    pub next_sequence: i64,
    pub event_count: i32,
    pub byte_count: i64,
    pub max_events: i32,
    pub max_bytes: i64,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub producer_lease_until: Option<DateTime<Utc>>,
    pub producer_epoch: i32,
}

impl AskStreamSession {
    pub fn high_water_sequence(&self) -> i64 {
        self.next_sequence.saturating_sub(1).max(0)
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self.status, AskStreamStatus::Open)
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AskStreamEvent {
    pub id: Uuid,
    pub org_id: Uuid,
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub sequence_no: i64,
    pub event_type: String,
    pub envelope_version: i32,
    pub data: JsonValue,
    pub payload_bytes: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewAskStreamSession {
    pub id: Uuid,
    pub version_mode: String,
    pub collection_ids: Vec<Uuid>,
    pub cited_document_ids: Vec<Uuid>,
    pub cited_version_ids: Vec<Uuid>,
    pub pinned_snapshot: JsonValue,
    pub max_events: i32,
    pub max_bytes: i64,
    pub ttl_secs: i64,
}

pub async fn create_session(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    input: NewAskStreamSession,
) -> Result<AskStreamSession, DbError> {
    if input.max_events <= 1
        || input.max_events > 8192
        || input.max_bytes <= 0
        || input.max_bytes > 1_048_576
        || input.ttl_secs <= 0
    {
        return Err(DbError::Config("invalid ask stream bounds".into()));
    }
    let expires_at = Utc::now() + Duration::seconds(input.ttl_secs);
    let lease_until = Utc::now() + Duration::seconds(PRODUCER_LEASE_SECS);
    let row = txn
        .query_one(
            "INSERT INTO ask_stream_sessions (
                id, org_id, user_id, status, version_mode,
                collection_ids, cited_document_ids, cited_version_ids,
                pinned_snapshot, next_sequence, event_count, byte_count,
                max_events, max_bytes, expires_at,
                producer_lease_until, producer_epoch
             ) VALUES (
                $1, $2, $3, 'open', $4,
                $5, $6, $7,
                $8, 1, 0, 0,
                $9, $10, $11,
                $12, 0
             )
             RETURNING id, org_id, user_id, status, close_reason, version_mode,
                       collection_ids, cited_document_ids, cited_version_ids,
                       pinned_snapshot, next_sequence, event_count, byte_count,
                       max_events, max_bytes, expires_at, created_at, closed_at,
                       producer_lease_until, producer_epoch",
            &[
                &input.id,
                &ctx.org_id(),
                &ctx.user_id(),
                &input.version_mode,
                &input.collection_ids,
                &input.cited_document_ids,
                &input.cited_version_ids,
                &input.pinned_snapshot,
                &input.max_events,
                &input.max_bytes,
                &expires_at,
                &lease_until,
            ],
        )
        .await?;
    map_session(&row)
}

pub async fn get_owned_session(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
) -> Result<AskStreamSession, DbError> {
    let row = txn
        .query_opt(
            "SELECT id, org_id, user_id, status, close_reason, version_mode,
                    collection_ids, cited_document_ids, cited_version_ids,
                    pinned_snapshot, next_sequence, event_count, byte_count,
                    max_events, max_bytes, expires_at, created_at, closed_at,
                    producer_lease_until, producer_epoch
             FROM ask_stream_sessions
             WHERE org_id = $1 AND id = $2 AND user_id = $3",
            &[&ctx.org_id(), &session_id, &ctx.user_id()],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    map_session(&row)
}

async fn lock_session(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
) -> Result<(), DbError> {
    let key = format!("askstream:{}:{}", ctx.org_id(), session_id);
    txn.execute("SELECT pg_advisory_xact_lock(hashtext($1))", &[&key])
        .await?;
    Ok(())
}

/// Family → principal → reload OrgContext → citation fence (auth logout lock order).
///
/// Returns a freshly loaded OrgContext (permissions + collection ACL) observed
/// under the locks so producer/tail never trust a stale extractor snapshot.
pub async fn fence_family_principal_and_citations(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
    family_id: Uuid,
    cited_document_ids: &[Uuid],
) -> Result<OrgContext, DbError> {
    lock_refresh_family(txn, family_id)
        .await
        .map_err(|_| DbError::Config("session_revoked".into()))?;
    let family_live = txn
        .query_opt(
            "SELECT 1
             FROM refresh_tokens
             WHERE org_id = $1
               AND family_id = $2
               AND revoked_at IS NULL
               AND expires_at > now()
             LIMIT 1",
            &[&org_id, &family_id],
        )
        .await?;
    if family_live.is_none() {
        return Err(DbError::Config("session_revoked".into()));
    }
    fence_principal_reload_and_citations(txn, org_id, user_id, cited_document_ids).await
}

/// Principal lock → reload OrgContext → citation FOR SHARE checks.
pub async fn fence_principal_reload_and_citations(
    txn: &Transaction<'_>,
    org_id: Uuid,
    user_id: Uuid,
    cited_document_ids: &[Uuid],
) -> Result<OrgContext, DbError> {
    authz_lock::lock_principal_authz(txn, org_id, user_id).await?;
    let disabled: Option<DateTime<Utc>> = txn
        .query_one(
            "SELECT disabled_at FROM users WHERE id = $1 FOR SHARE",
            &[&user_id],
        )
        .await?
        .get(0);
    if disabled.is_some() {
        return Err(DbError::Config("principal_denied".into()));
    }
    let membership = txn
        .query_opt(
            "SELECT 1 FROM org_memberships
             WHERE org_id = $1 AND user_id = $2
             FOR SHARE",
            &[&org_id, &user_id],
        )
        .await?;
    if membership.is_none() {
        return Err(DbError::Config("principal_denied".into()));
    }
    let ctx = resolve_org_context_on_txn(txn, org_id, user_id)
        .await
        .map_err(|error| match error {
            ResolveError::UserDisabled | ResolveError::MembershipMissing => {
                DbError::Config("principal_denied".into())
            }
            ResolveError::PermissionDenied | ResolveError::CollectionDenied => {
                DbError::Config("citation_revoked".into())
            }
            _ => DbError::Config("principal_denied".into()),
        })?;
    // Ask stream requires qa.query on every content pull/append (fresh under lock).
    if !cited_document_ids.is_empty() && !ctx.has_permission("qa.query") {
        return Err(DbError::Config("principal_denied".into()));
    }
    let tombstoned = DocumentState::Tombstoned.as_str();
    for document_id in cited_document_ids {
        let row = txn
            .query_opt(
                "SELECT collection_id, state, deleted_at
                 FROM documents
                 WHERE org_id = $1 AND id = $2
                 FOR SHARE",
                &[&org_id, document_id],
            )
            .await?;
        let Some(row) = row else {
            return Err(DbError::Config("citation_revoked".into()));
        };
        let collection_id: Uuid = row.get(0);
        let state: String = row.get(1);
        let deleted_at: Option<DateTime<Utc>> = row.get(2);
        if deleted_at.is_some() || state == tombstoned || !ctx.allows_collection(collection_id) {
            return Err(DbError::Config("citation_revoked".into()));
        }
    }
    Ok(ctx)
}

/// Principal + citation fence under the shared authz lock (closes TOCTOU).
pub async fn fence_principal_and_citations(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    cited_document_ids: &[Uuid],
) -> Result<OrgContext, DbError> {
    fence_principal_reload_and_citations(txn, ctx.org_id(), ctx.user_id(), cited_document_ids).await
}

fn is_terminal_event_type(event_type: &str) -> bool {
    event_type == TERMINAL_EVENT_TYPE
}

/// Append with family+principal+citation fence immediately before write.
///
/// Uses the freshly reloaded OrgContext from the fence (not the caller's stale copy).
pub async fn append_event_authorized(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    family_id: Uuid,
    session_id: Uuid,
    event_type: &str,
    data: JsonValue,
    cited_document_ids: &[Uuid],
) -> Result<AskStreamEvent, DbError> {
    let fresh = fence_family_principal_and_citations(
        txn,
        ctx.org_id(),
        ctx.user_id(),
        family_id,
        cited_document_ids,
    )
    .await?;
    append_event_locked(txn, &fresh, session_id, event_type, data).await
}

pub async fn append_event(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
    event_type: &str,
    data: JsonValue,
) -> Result<AskStreamEvent, DbError> {
    // Legacy entrypoint: still takes session lock; prefer append_event_authorized.
    append_event_locked(txn, ctx, session_id, event_type, data).await
}

async fn append_event_locked(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
    event_type: &str,
    data: JsonValue,
) -> Result<AskStreamEvent, DbError> {
    lock_session(txn, ctx, session_id).await?;
    let session = get_owned_session(txn, ctx, session_id).await?;
    if session.status != AskStreamStatus::Open {
        return Err(DbError::Config("ask stream session is not open".into()));
    }
    if session.is_expired(Utc::now()) {
        return Err(DbError::Config("ask stream session expired".into()));
    }
    let payload = serde_json::to_vec(&data).unwrap_or_default();
    let payload_bytes = i32::try_from(payload.len()).unwrap_or(i32::MAX);
    if payload_bytes > MAX_EVENT_PAYLOAD_BYTES {
        return Err(DbError::Config("ask stream event payload too large".into()));
    }
    // Reserve one slot for the durable terminal event unless this is the terminal.
    let reserve_terminal = if is_terminal_event_type(event_type) {
        0
    } else {
        1
    };
    if session.event_count + reserve_terminal >= session.max_events {
        return Err(DbError::Config("ask stream exceeds max_events".into()));
    }
    let next_bytes = session.byte_count.saturating_add(i64::from(payload_bytes));
    if next_bytes > session.max_bytes {
        return Err(DbError::Config("ask stream exceeds max_bytes".into()));
    }
    let sequence_no = session.next_sequence;
    let lease_until = Utc::now() + Duration::seconds(PRODUCER_LEASE_SECS);
    let row = txn
        .query_one(
            "INSERT INTO ask_stream_events (
                org_id, session_id, user_id, sequence_no, event_type,
                envelope_version, data, payload_bytes
             ) VALUES ($1, $2, $3, $4, $5, 1, $6, $7)
             RETURNING id, org_id, session_id, user_id, sequence_no, event_type,
                       envelope_version, data, payload_bytes, created_at",
            &[
                &ctx.org_id(),
                &session_id,
                &ctx.user_id(),
                &sequence_no,
                &event_type,
                &data,
                &payload_bytes,
            ],
        )
        .await?;
    txn.execute(
        "UPDATE ask_stream_sessions
         SET next_sequence = $4,
             event_count = event_count + 1,
             byte_count = $5,
             producer_lease_until = $6
         WHERE org_id = $1 AND id = $2 AND user_id = $3 AND status = 'open'",
        &[
            &ctx.org_id(),
            &session_id,
            &ctx.user_id(),
            &(sequence_no + 1),
            &next_bytes,
            &lease_until,
        ],
    )
    .await?;
    map_event(&row)
}

/// Exactly one durable terminal: append `stream.closed` + close session, or return
/// the existing terminal event when the session is already closed.
pub async fn close_with_terminal(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    family_id: Option<Uuid>,
    session_id: Uuid,
    status: AskStreamStatus,
    reason: &str,
    cited_document_ids: &[Uuid],
) -> Result<Option<AskStreamEvent>, DbError> {
    if matches!(status, AskStreamStatus::Open) {
        return Err(DbError::Config("cannot close session as open".into()));
    }
    // Best-effort fence; if principal/family is already gone, still close without content.
    let fenced = if let Some(family_id) = family_id {
        fence_family_principal_and_citations(
            txn,
            ctx.org_id(),
            ctx.user_id(),
            family_id,
            cited_document_ids,
        )
        .await
    } else {
        fence_principal_and_citations(txn, ctx, cited_document_ids).await
    };
    let write_ctx = fenced.as_ref().unwrap_or(ctx);
    lock_session(txn, write_ctx, session_id).await?;
    let session = get_owned_session(txn, write_ctx, session_id).await?;
    if session.is_terminal() {
        let existing = txn
            .query_opt(
                "SELECT id, org_id, session_id, user_id, sequence_no, event_type,
                        envelope_version, data, payload_bytes, created_at
                 FROM ask_stream_events
                 WHERE org_id = $1 AND session_id = $2 AND user_id = $3
                   AND event_type = $4
                 ORDER BY sequence_no DESC
                 LIMIT 1",
                &[
                    &write_ctx.org_id(),
                    &session_id,
                    &write_ctx.user_id(),
                    &TERMINAL_EVENT_TYPE,
                ],
            )
            .await?;
        return existing.as_ref().map(map_event).transpose();
    }
    let data = serde_json::json!({
        "reason": reason,
        "streamSessionId": session_id,
    });
    // Allocate durable terminal via sequence lock (content payload is reason only).
    let event = append_event_locked(txn, write_ctx, session_id, TERMINAL_EVENT_TYPE, data).await?;
    txn.execute(
        "UPDATE ask_stream_sessions
         SET status = $4,
             close_reason = $5,
             closed_at = clock_timestamp(),
             producer_lease_until = NULL
         WHERE org_id = $1 AND id = $2 AND user_id = $3 AND status = 'open'",
        &[
            &ctx.org_id(),
            &session_id,
            &ctx.user_id(),
            &status.as_str(),
            &reason,
        ],
    )
    .await?;
    Ok(Some(event))
}

pub async fn close_session(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
    status: AskStreamStatus,
    close_reason: &str,
) -> Result<AskStreamSession, DbError> {
    let _ = close_with_terminal(txn, ctx, None, session_id, status, close_reason, &[]).await?;
    get_owned_session(txn, ctx, session_id).await
}

pub async fn list_events_after(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
    after_sequence: i64,
    limit: i64,
) -> Result<Vec<AskStreamEvent>, DbError> {
    let rows = txn
        .query(
            "SELECT id, org_id, session_id, user_id, sequence_no, event_type,
                    envelope_version, data, payload_bytes, created_at
             FROM ask_stream_events
             WHERE org_id = $1 AND session_id = $2 AND user_id = $3
               AND sequence_no > $4
             ORDER BY sequence_no ASC
             LIMIT $5",
            &[
                &ctx.org_id(),
                &session_id,
                &ctx.user_id(),
                &after_sequence,
                &limit,
            ],
        )
        .await?;
    rows.iter().map(map_event).collect()
}

/// Purge expired sessions via SECURITY DEFINER SKIP LOCKED cascade. Bounded.
pub async fn purge_expired_sessions(
    client: &tokio_postgres::Client,
    limit: i64,
) -> Result<(u64, u64), DbError> {
    let limit = i32::try_from(limit.clamp(1, 500)).unwrap_or(100);
    let row = client
        .query_one(
            "SELECT sessions_purged, events_purged
             FROM markhand_purge_expired_ask_streams($1)",
            &[&limit],
        )
        .await?;
    let sessions: i64 = row.get(0);
    let events: i64 = row.get(1);
    let sessions = u64::try_from(sessions).unwrap_or(0);
    let events = u64::try_from(events).unwrap_or(0);
    PURGED_SESSIONS.fetch_add(sessions, Ordering::Relaxed);
    PURGED_EVENTS.fetch_add(events, Ordering::Relaxed);
    Ok((sessions, events))
}

/// Recover stale producers via SECURITY DEFINER (durable terminal once).
pub async fn recover_stale_producers(
    client: &tokio_postgres::Client,
    limit: i64,
) -> Result<u64, DbError> {
    let limit = i32::try_from(limit.clamp(1, 100)).unwrap_or(50);
    let recovered: i64 = client
        .query_one(
            "SELECT markhand_recover_stale_ask_stream_producers($1)",
            &[&limit],
        )
        .await?
        .get(0);
    let recovered = u64::try_from(recovered).unwrap_or(0);
    PRODUCER_RECOVERED.fetch_add(recovered, Ordering::Relaxed);
    Ok(recovered)
}

/// Pool helper for maintenance sweeps (no org context; definer functions).
pub async fn run_maintenance(
    pool: &deadpool_postgres::Pool,
    limit: i64,
) -> Result<(u64, u64, u64), DbError> {
    let client = pool.get().await?;
    let (sessions, events) = purge_expired_sessions(&client, limit).await?;
    let recovered = recover_stale_producers(&client, limit).await?;
    Ok((sessions, events, recovered))
}

fn map_session(row: &Row) -> Result<AskStreamSession, DbError> {
    Ok(AskStreamSession {
        id: row.get("id"),
        org_id: row.get("org_id"),
        user_id: row.get("user_id"),
        status: AskStreamStatus::parse(row.get("status"))?,
        close_reason: row.get("close_reason"),
        version_mode: row.get("version_mode"),
        collection_ids: row.get("collection_ids"),
        cited_document_ids: row.get("cited_document_ids"),
        cited_version_ids: row.get("cited_version_ids"),
        pinned_snapshot: row.get("pinned_snapshot"),
        next_sequence: row.get("next_sequence"),
        event_count: row.get("event_count"),
        byte_count: row.get("byte_count"),
        max_events: row.get("max_events"),
        max_bytes: row.get("max_bytes"),
        expires_at: row.get("expires_at"),
        created_at: row.get("created_at"),
        closed_at: row.get("closed_at"),
        producer_lease_until: row.try_get("producer_lease_until").ok().flatten(),
        producer_epoch: row.try_get("producer_epoch").unwrap_or(0),
    })
}

fn map_event(row: &Row) -> Result<AskStreamEvent, DbError> {
    Ok(AskStreamEvent {
        id: row.get("id"),
        org_id: row.get("org_id"),
        session_id: row.get("session_id"),
        user_id: row.get("user_id"),
        sequence_no: row.get("sequence_no"),
        event_type: row.get("event_type"),
        envelope_version: row.get("envelope_version"),
        data: row.get("data"),
        payload_bytes: row.get("payload_bytes"),
        created_at: row.get("created_at"),
    })
}
