//! Closed-snapshot SSE persistence and delivery auth (P1B-R05).
//!
//! Routes parse/map HTTP only; this service owns DB txn wiring for snapshots
//! and per-event auth/pin/session probes (ADR 0001).

mod auth;
mod plan;

use std::collections::BTreeSet;

use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;
use crate::db::pool::with_org_txn;
use crate::db::sse_streams::{
    self, NewClosedSnapshot, SseStreamKind, SseStreamStatus, DEFAULT_CLEANUP_LIMIT,
    DEFAULT_TTL_SECS,
};
use crate::services::qa::QaAnswer;
use crate::services::retrieval::VersionMode;

pub use crate::db::sse_streams::{
    ClosedSnapshotLoad, SseStreamEvent, SseStreamRequest, StreamAuthScope, DEFAULT_MAX_BYTES,
    DEFAULT_MAX_EVENTS,
};
pub use auth::{make_auth_probe, probe_cited_pins};
pub(crate) use plan::json_payload_bytes;
pub use plan::{citation_to_json, metadata_data, plan_closed_events, SnapshotPlanBounds};

pub fn version_mode_label(mode: &VersionMode) -> &'static str {
    match mode {
        VersionMode::Current => "current",
        VersionMode::AsOf { .. } => "as_of",
        VersionMode::Compare { .. } => "compare",
        VersionMode::History { .. } => "history",
    }
}

pub fn mode_requires_history(mode: &VersionMode) -> bool {
    !matches!(mode, VersionMode::Current)
}

pub fn build_auth_scope(
    mode: &VersionMode,
    collection_ids: Vec<Uuid>,
    answer: &QaAnswer,
) -> StreamAuthScope {
    let mut docs = BTreeSet::new();
    let mut versions = BTreeSet::new();
    for citation in &answer.citations {
        docs.insert(citation.document_id);
        versions.insert(citation.version_id);
    }
    for id in &answer.version_context.cited_version_ids {
        versions.insert(*id);
    }
    StreamAuthScope {
        version_mode: version_mode_label(mode).to_string(),
        requires_history: mode_requires_history(mode),
        collection_ids,
        cited_document_ids: docs.into_iter().collect(),
        cited_version_ids: versions.into_iter().collect(),
    }
}

/// Input for persisting a planned ask closed snapshot.
pub struct PersistAskSnapshot {
    pub stream_id: Uuid,
    pub auth_scope: StreamAuthScope,
    pub planned: Vec<sse_streams::PlannedSseEvent>,
    pub close_reason: &'static str,
    pub max_events: i32,
    pub max_bytes: i64,
}

/// Persist a planned ask snapshot after opportunistic expired cleanup.
pub async fn persist_ask_closed_snapshot(
    pool: &Pool,
    ctx: &OrgContext,
    input: PersistAskSnapshot,
) -> Result<(SseStreamRequest, Vec<SseStreamEvent>), DbError> {
    let status = if input.close_reason == "completed" {
        SseStreamStatus::Closed
    } else {
        SseStreamStatus::Error
    };
    let ctx = ctx.clone();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                let _ = sse_streams::cleanup_expired(
                    txn,
                    &ctx,
                    DEFAULT_CLEANUP_LIMIT,
                    Some(input.stream_id),
                )
                .await?;
                sse_streams::persist_closed_snapshot(
                    txn,
                    &ctx,
                    NewClosedSnapshot {
                        id: input.stream_id,
                        kind: SseStreamKind::Ask,
                        status,
                        close_reason: input.close_reason,
                        auth_scope: input.auth_scope,
                        events: input.planned,
                        max_events: input.max_events,
                        max_bytes: input.max_bytes,
                        ttl_secs: DEFAULT_TTL_SECS,
                    },
                )
                .await
            })
        }
    })
    .await
}

/// Load owned closed snapshot with DB-time expiry distinction.
pub async fn load_owned_closed_snapshot(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: Uuid,
) -> Result<ClosedSnapshotLoad, DbError> {
    let ctx = ctx.clone();
    with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(
                async move { sse_streams::load_owned_closed_request(txn, &ctx, request_id).await },
            )
        }
    })
    .await
}

/// Expire + delete one stream, then bounded opportunistic cleanup.
pub async fn expire_stream_and_cleanup(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: Uuid,
) -> Result<(), DbError> {
    let ctx = ctx.clone();
    let _ = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move { sse_streams::expire_and_delete(txn, &ctx, request_id).await })
        }
    })
    .await;
    let _ = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                sse_streams::cleanup_expired(txn, &ctx, DEFAULT_CLEANUP_LIMIT, None).await
            })
        }
    })
    .await;
    Ok(())
}

/// List events after a sequence, then cleanup expired peers (excluding active).
pub async fn list_events_after_and_cleanup(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: Uuid,
    after_sequence: u64,
) -> Result<Vec<SseStreamEvent>, DbError> {
    let ctx = ctx.clone();
    let events = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                sse_streams::list_events_after(txn, &ctx, request_id, after_sequence).await
            })
        }
    })
    .await?;
    let _ = with_org_txn(pool, &ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                sse_streams::cleanup_expired(txn, &ctx, DEFAULT_CLEANUP_LIMIT, Some(request_id))
                    .await
            })
        }
    })
    .await;
    Ok(events)
}

/// Convenience defaults for ask-stream persistence bounds.
pub fn default_snapshot_plan_bounds() -> SnapshotPlanBounds {
    SnapshotPlanBounds {
        max_events: DEFAULT_MAX_EVENTS,
        max_bytes: DEFAULT_MAX_BYTES,
        ..SnapshotPlanBounds::default()
    }
}
