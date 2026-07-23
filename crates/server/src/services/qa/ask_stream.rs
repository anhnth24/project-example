//! Durable ask SSE producer + live-tail delivery (P1B-R05).
//!
//! Flow:
//! 1. Run retrieval once and pin ID/hash snapshot (no question/answer/quote body).
//! 2. Spawn a producer that streams provider tokens or extractive chunks, appending
//!    each envelope under principal+citation fence (shared authz lock).
//! 3. SSE handlers live-tail durable events; reconnect uses Last-Event-ID and
//!    never re-runs retrieval/provider. Synthetic closes are either durable
//!    terminals (sequence allocator) or control frames without SSE id.

use std::collections::BTreeSet;
use std::time::Duration;

use deadpool_postgres::Pool;
use fileconv_knowledge::ask::{extractive_answer, AnswerMode};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::SseEnvelope;
use crate::auth::context::OrgContext;
use crate::auth::jwt::AccessClaims;
use crate::db::ask_streams::{
    self, AskStreamStatus, NewAskStreamSession, DEFAULT_MAX_BYTES, DEFAULT_MAX_EVENTS,
    DEFAULT_TTL_SECS, TERMINAL_EVENT_TYPE,
};
use crate::db::pool::{apply_org_context, with_org_txn};
use crate::services::citation::{pins_from_hits, CitationPin};
use crate::services::embedding::ApprovedEmbeddingRuntime;
use crate::services::qa::grounding::{
    conflict_resolution_notes_for_history, conflict_warnings_for_current, version_context_note,
    VersionContext,
};
use crate::services::qa::prompt::build_grounded_messages;
use crate::services::qa::provider::{ChatProvider, ProviderError, StreamCancel};
use crate::services::qa::stream::{tokenize_answer, HEARTBEAT_INTERVAL, SSE_ENVELOPE_VERSION};
use crate::services::qa::{force_extractive_only_runtime, hits_to_hybrid};
use crate::services::retrieval::{hybrid_search, RetrievalHit, RetrievalRequest, VersionMode};
use crate::services::stream_auth::{self, StreamAuthError};
use crate::storage::qdrant::QdrantClient;

pub const ASK_STREAM_SEND_TIMEOUT: Duration = Duration::from_secs(5);
pub const ASK_STREAM_POLL_IDLE: Duration = Duration::from_millis(200);
/// Pull at most one durable event per authorize cycle (no prebuffered batches).
pub const ASK_STREAM_BATCH: i64 = 1;
/// Fixed total deadline for reauthorize + select under DB locks (then release).
pub const ASK_STREAM_PULL_DEADLINE: Duration = Duration::from_secs(2);
/// Single-item channel: send never runs while locks are held; no multi-event buffer.
pub const ASK_STREAM_CHANNEL_CAP: usize = 1;

#[derive(Debug, Clone)]
pub struct AskStreamStart {
    pub session_id: Uuid,
    pub cited_document_ids: Vec<Uuid>,
    pub cancel: StreamCancel,
}

#[derive(Debug)]
pub enum AskStreamPrepareError {
    Retrieval(crate::services::retrieval::RetrievalError),
    InvalidRequest(&'static str),
    Database,
}

/// Prepare retrieval, create durable session, spawn producer. Returns session id.
#[allow(clippy::too_many_arguments)]
pub async fn start_ask_stream(
    pool: &Pool,
    qdrant: &QdrantClient,
    embedder: Option<&ApprovedEmbeddingRuntime>,
    provider: Option<ChatProvider>,
    ctx: &OrgContext,
    claims: AccessClaims,
    request_id: String,
    question: String,
    collection_ids: Option<BTreeSet<Uuid>>,
    mode: VersionMode,
    limit: usize,
    conflict_ids: Vec<Uuid>,
) -> Result<AskStreamStart, AskStreamPrepareError> {
    if question.trim().is_empty() {
        return Err(AskStreamPrepareError::InvalidRequest("question is empty"));
    }
    if question.len() > 8_192 {
        return Err(AskStreamPrepareError::InvalidRequest(
            "question exceeds max length",
        ));
    }
    let retrieval = hybrid_search(
        pool,
        qdrant,
        embedder,
        ctx,
        RetrievalRequest {
            query: question.clone(),
            collection_ids: collection_ids.clone(),
            mode: mode.clone(),
            limit: limit.clamp(1, 20),
            conflict_ids: conflict_ids.clone(),
        },
    )
    .await
    .map_err(AskStreamPrepareError::Retrieval)?;

    let citations = pins_from_hits(ctx.org_id(), &retrieval.hits);
    let cited_document_ids: Vec<Uuid> = citations
        .iter()
        .map(|pin| pin.logical_document_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let cited_version_ids: Vec<Uuid> = citations
        .iter()
        .map(|pin| pin.version_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let collection_list: Vec<Uuid> = collection_ids
        .clone()
        .unwrap_or_else(|| ctx.allowed_collection_ids().iter().copied().collect())
        .into_iter()
        .collect();
    let version_context = version_context_note(&mode, &citations, &retrieval.hits);
    let mut warnings = retrieval.warnings;
    warnings.extend(conflict_warnings_for_current(
        &mode,
        &retrieval.conflict_evidence,
    ));
    warnings.extend(conflict_resolution_notes_for_history(
        &mode,
        &retrieval.conflict_evidence,
    ));

    let hybrid = hits_to_hybrid(&retrieval.hits);
    let extractive = extractive_answer(&question, &hybrid);
    let session_id = Uuid::new_v4();
    // Retention-safe snapshot: IDs/hashes only — never question/answer/quote body.
    let pinned_snapshot = json!({
        "embeddingMode": retrieval.embedding_mode,
        "hitIds": retrieval.hits.iter().map(hit_id_summary).collect::<Vec<_>>(),
        "citationIds": citations.iter().map(|pin| json!({
            "documentId": pin.logical_document_id,
            "versionId": pin.version_id,
            "chunkIdentitySha256": pin.chunk_identity_sha256,
        })).collect::<Vec<_>>(),
        "versionMode": mode_wire(&mode),
        "warningCount": warnings.len(),
        "extractiveChars": extractive.chars().count(),
        "questionSha256": sha256_hex(question.as_bytes()),
    });

    let version_mode = mode_wire(&mode);
    let ctx_owned = ctx.clone();
    with_org_txn(pool, ctx, {
        let pinned = pinned_snapshot.clone();
        let collections = collection_list.clone();
        let cited_docs = cited_document_ids.clone();
        let cited_versions = cited_version_ids.clone();
        let version_mode = version_mode.to_string();
        move |txn| {
            Box::pin(async move {
                ask_streams::create_session(
                    txn,
                    &ctx_owned,
                    NewAskStreamSession {
                        id: session_id,
                        version_mode,
                        collection_ids: collections,
                        cited_document_ids: cited_docs,
                        cited_version_ids: cited_versions,
                        pinned_snapshot: pinned,
                        max_events: DEFAULT_MAX_EVENTS,
                        max_bytes: DEFAULT_MAX_BYTES,
                        ttl_secs: DEFAULT_TTL_SECS,
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .map_err(|_| AskStreamPrepareError::Database)?;

    let cancel = StreamCancel::new();
    let producer_pool = pool.clone();
    let producer_ctx = ctx.clone();
    let producer_cancel = cancel.clone();
    let producer_provider = provider;
    let producer_cited = cited_document_ids.clone();
    tokio::spawn(async move {
        run_producer(
            producer_pool,
            producer_ctx,
            claims,
            session_id,
            request_id,
            citations,
            producer_cited,
            version_context,
            warnings,
            extractive,
            retrieval.embedding_mode,
            retrieval.hits,
            question,
            mode,
            producer_provider,
            producer_cancel,
        )
        .await;
    });

    Ok(AskStreamStart {
        session_id,
        cited_document_ids,
        cancel,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run_producer(
    pool: Pool,
    ctx: OrgContext,
    claims: AccessClaims,
    session_id: Uuid,
    _request_id: String,
    citations: Vec<CitationPin>,
    cited_document_ids: Vec<Uuid>,
    version_context: VersionContext,
    mut warnings: Vec<String>,
    extractive: String,
    embedding_mode: String,
    hits: Vec<RetrievalHit>,
    question: String,
    mode: VersionMode,
    provider: Option<ChatProvider>,
    cancel: StreamCancel,
) {
    let family_id = Uuid::parse_str(&claims.sid).ok();

    let append = |event_type: &'static str, data: Value| {
        let pool = pool.clone();
        let ctx = ctx.clone();
        let claims = claims.clone();
        let cited = cited_document_ids.clone();
        async move {
            // JWT exp check outside txn; family+principal+citation fence inside append.
            if stream_auth::token_expired(&claims, chrono::Utc::now().timestamp()) {
                return Err(crate::db::error::DbError::Config("token_expired".into()));
            }
            let Some(family_id) = Uuid::parse_str(&claims.sid).ok() else {
                return Err(crate::db::error::DbError::Config("session_revoked".into()));
            };
            with_org_txn(&pool, &ctx, {
                let ctx = ctx.clone();
                move |txn| {
                    Box::pin(async move {
                        ask_streams::append_event_authorized(
                            txn, &ctx, family_id, session_id, event_type, data, &cited,
                        )
                        .await?;
                        Ok(())
                    })
                }
            })
            .await
        }
    };

    let close = |status: AskStreamStatus, reason: &'static str| {
        let pool = pool.clone();
        let ctx = ctx.clone();
        let cited = cited_document_ids.clone();
        async move {
            let _ = with_org_txn(&pool, &ctx, {
                let ctx = ctx.clone();
                move |txn| {
                    Box::pin(async move {
                        ask_streams::close_with_terminal(
                            txn, &ctx, family_id, session_id, status, reason, &cited,
                        )
                        .await?;
                        Ok(())
                    })
                }
            })
            .await;
        }
    };

    if let Err(error) = append(
        "ask.started",
        json!({
            "streamSessionId": session_id,
            "mode": AnswerMode::OfflineExtractive.as_str(),
            "embeddingMode": embedding_mode,
            "citationCount": citations.len(),
        }),
    )
    .await
    {
        let reason = config_reason(&error).unwrap_or("stream_error");
        close(AskStreamStatus::Error, reason).await;
        return;
    }

    let use_provider_stream = provider
        .as_ref()
        .is_some_and(|p| p.supports_incremental_stream() && !force_extractive_only_runtime());

    let mut answer_mode = AnswerMode::OfflineExtractive;
    let mut streamed_any = false;

    if use_provider_stream {
        if let Some(chat) = provider.as_ref() {
            let hybrid = hits_to_hybrid(&hits);
            let messages = build_grounded_messages(&question, &hybrid, &mode);
            match chat.stream_tokens(&messages, cancel.clone()).await {
                Ok(mut rx) => {
                    answer_mode = chat.answer_mode();
                    while let Some(item) = rx.recv().await {
                        if cancel.is_cancelled() {
                            close(AskStreamStatus::Error, "cancelled").await;
                            return;
                        }
                        match item {
                            Ok(token) => {
                                streamed_any = true;
                                if let Err(error) =
                                    append("ask.token", json!({ "text": token })).await
                                {
                                    let reason = config_reason(&error).unwrap_or("stream_error");
                                    cancel.cancel();
                                    close(AskStreamStatus::Error, reason).await;
                                    return;
                                }
                            }
                            Err(ProviderError::Cancelled) => {
                                close(AskStreamStatus::Error, "cancelled").await;
                                return;
                            }
                            Err(ProviderError::Timeout) => {
                                warnings.push(
                                    "LLM provider timed out; using extractive fallback.".into(),
                                );
                                answer_mode = AnswerMode::FallbackExtractive;
                                break;
                            }
                            Err(_) => {
                                warnings.push(
                                    "LLM provider unavailable; using extractive fallback.".into(),
                                );
                                answer_mode = AnswerMode::FallbackExtractive;
                                break;
                            }
                        }
                    }
                }
                Err(ProviderError::Timeout) => {
                    warnings.push("LLM provider timed out; using extractive fallback.".into());
                    answer_mode = AnswerMode::FallbackExtractive;
                }
                Err(_) => {
                    warnings.push("LLM provider unavailable; using extractive fallback.".into());
                    answer_mode = AnswerMode::FallbackExtractive;
                }
            }
        }
    }

    if !streamed_any || matches!(answer_mode, AnswerMode::FallbackExtractive) {
        if force_extractive_only_runtime() {
            warnings.push(
                "Structured entailment unavailable; fail-closed extractive-only grounding.".into(),
            );
        }
        answer_mode = if matches!(answer_mode, AnswerMode::FallbackExtractive) {
            AnswerMode::FallbackExtractive
        } else {
            AnswerMode::OfflineExtractive
        };
        for token in tokenize_answer(&extractive) {
            if cancel.is_cancelled() {
                close(AskStreamStatus::Error, "cancelled").await;
                return;
            }
            if let Err(error) = append("ask.token", json!({ "text": token })).await {
                let reason = config_reason(&error).unwrap_or("stream_error");
                cancel.cancel();
                close(AskStreamStatus::Error, reason).await;
                return;
            }
        }
    }

    for warning in &warnings {
        if let Err(error) = append("ask.warning", json!({ "message": warning })).await {
            let reason = config_reason(&error).unwrap_or("stream_error");
            close(AskStreamStatus::Error, reason).await;
            return;
        }
    }
    if let Err(error) = append("ask.citations", json!({ "citations": citations })).await {
        let reason = config_reason(&error).unwrap_or("stream_error");
        close(AskStreamStatus::Error, reason).await;
        return;
    }
    if let Err(error) = append("ask.version_context", json!(version_context)).await {
        let reason = config_reason(&error).unwrap_or("stream_error");
        close(AskStreamStatus::Error, reason).await;
        return;
    }
    if let Err(error) = append(
        "ask.completed",
        json!({
            "mode": answer_mode.as_str(),
            "streamSessionId": session_id,
        }),
    )
    .await
    {
        let reason = config_reason(&error).unwrap_or("stream_error");
        close(AskStreamStatus::Error, reason).await;
        return;
    }
    close(AskStreamStatus::Closed, "completed").await;
}

fn config_reason(error: &crate::db::error::DbError) -> Option<&'static str> {
    match error {
        crate::db::error::DbError::Config(message) => match message.as_str() {
            "token_expired" => Some("token_expired"),
            "session_revoked" => Some("session_revoked"),
            "principal_denied" => Some("principal_denied"),
            "citation_revoked" => Some("citation_revoked"),
            "ask stream session expired" => Some("session_expired"),
            _ => Some("stream_error"),
        },
        _ => None,
    }
}

fn hit_id_summary(hit: &RetrievalHit) -> Value {
    json!({
        "documentId": hit.document_id,
        "versionId": hit.version_id,
        "versionNumber": hit.version_number,
        "isCurrent": hit.is_current,
        "chunkIdentitySha256": hit.chunk_identity_sha256,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn mode_wire(mode: &VersionMode) -> &'static str {
    match mode {
        VersionMode::Current => "current",
        VersionMode::AsOf { .. } => "as_of",
        VersionMode::Compare { .. } => "compare",
        VersionMode::History { .. } => "history",
    }
}

/// Live-tail durable ask events with auth revalidation and slow-client bounds.
pub async fn live_tail_ask_session(
    pool: Pool,
    claims: AccessClaims,
    session_id: Uuid,
    request_id: String,
    cited_document_ids: Vec<Uuid>,
    mut after_sequence: i64,
    cancel: Option<StreamCancel>,
) -> tokio::sync::mpsc::Receiver<Result<axum::response::sse::Event, std::convert::Infallible>> {
    // Capacity 1: never prebuffer a batch that could survive a later revoke.
    let (tx, rx) = tokio::sync::mpsc::channel(ASK_STREAM_CHANNEL_CAP);
    tokio::spawn(async move {
        let mut idle_polls = 0_u32;
        let Ok(family_id) = Uuid::parse_str(&claims.sid) else {
            let _ = send_control_closed(&tx, &request_id, "session_revoked").await;
            return;
        };
        let Ok(org_id) = Uuid::parse_str(&claims.org_id) else {
            let _ = send_control_closed(&tx, &request_id, "principal_denied").await;
            return;
        };
        let Ok(user_id) = Uuid::parse_str(&claims.sub) else {
            let _ = send_control_closed(&tx, &request_id, "principal_denied").await;
            return;
        };
        loop {
            if cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
                let _ = durable_or_control_close(
                    &pool,
                    &claims,
                    session_id,
                    &cited_document_ids,
                    &request_id,
                    "cancelled",
                    AskStreamStatus::Error,
                    &tx,
                )
                .await;
                return;
            }

            if stream_auth::token_expired(&claims, chrono::Utc::now().timestamp()) {
                let _ = durable_or_control_close(
                    &pool,
                    &claims,
                    session_id,
                    &cited_document_ids,
                    &request_id,
                    "token_expired",
                    AskStreamStatus::Error,
                    &tx,
                )
                .await;
                return;
            }

            // Reserve the sole channel slot before any DB work. Cap=1 means this
            // awaits client drain of the previous event with no locks held — so we
            // never select/enqueue while prior content is still buffered, and
            // logout/delete writers are never blocked behind SSE send I/O.
            let permit = match tokio::time::timeout(ASK_STREAM_SEND_TIMEOUT, tx.reserve()).await {
                Ok(Ok(permit)) => permit,
                Ok(Err(_)) => {
                    if let Some(cancel) = &cancel {
                        cancel.cancel();
                    }
                    return;
                }
                Err(_) => {
                    let _ = send_control_closed(
                        &tx,
                        &request_id,
                        StreamAuthError::SendTimeout.close_reason(),
                    )
                    .await;
                    if let Some(cancel) = &cancel {
                        cancel.cancel();
                    }
                    return;
                }
            };

            // Critical section under a fixed deadline: family→principal→reload
            // OrgContext→select at most one event, then commit/release. Enqueue
            // via permit is non-blocking (never await client under locks).
            let pull = tokio::time::timeout(ASK_STREAM_PULL_DEADLINE, async {
                let provisional = OrgContext::try_new(org_id, user_id, [] as [&str; 0], [])
                    .map_err(|_| crate::db::error::DbError::Config("principal_denied".into()))?;
                let mut client = pool.get().await?;
                let txn = client.transaction().await?;
                apply_org_context(&txn, &provisional).await?;
                let fresh = ask_streams::fence_family_principal_and_citations(
                    &txn,
                    org_id,
                    user_id,
                    family_id,
                    &cited_document_ids,
                )
                .await?;
                let session = ask_streams::get_owned_session(&txn, &fresh, session_id).await?;
                let expired = session.is_expired(chrono::Utc::now());
                let terminal = session.is_terminal();
                let mut events = ask_streams::list_events_after(
                    &txn,
                    &fresh,
                    session_id,
                    after_sequence,
                    ASK_STREAM_BATCH,
                )
                .await?;
                let event = events.pop(); // limit=1 ASC → sole next event
                txn.commit().await?;
                Ok::<_, crate::db::error::DbError>((expired, terminal, event))
            })
            .await;

            let (expired, session_terminal, event) = match pull {
                Err(_) => {
                    drop(permit);
                    let _ = send_control_closed(&tx, &request_id, "stream_error").await;
                    return;
                }
                Ok(Err(error)) => {
                    drop(permit);
                    let reason = config_reason(&error).unwrap_or("stream_error");
                    if let Some(cancel) = &cancel {
                        cancel.cancel();
                    }
                    let _ = durable_or_control_close(
                        &pool,
                        &claims,
                        session_id,
                        &cited_document_ids,
                        &request_id,
                        reason,
                        AskStreamStatus::Error,
                        &tx,
                    )
                    .await;
                    return;
                }
                Ok(Ok(tuple)) => tuple,
            };

            if expired {
                drop(permit);
                let _ = durable_or_control_close(
                    &pool,
                    &claims,
                    session_id,
                    &cited_document_ids,
                    &request_id,
                    "session_expired",
                    AskStreamStatus::Error,
                    &tx,
                )
                .await;
                return;
            }

            let Some(event) = event else {
                drop(permit);
                idle_polls += 1;
                if session_terminal {
                    return;
                }
                if idle_polls >= 150 {
                    let _ = send_control_closed(&tx, &request_id, "live_tail_timeout").await;
                    return;
                }
                tokio::time::sleep(ASK_STREAM_POLL_IDLE).await;
                continue;
            };
            idle_polls = 0;

            after_sequence = event.sequence_no;
            let event_type = event.event_type.clone();
            let envelope = SseEnvelope {
                version: SSE_ENVELOPE_VERSION,
                sequence: event.sequence_no as u64,
                event: event.event_type.clone(),
                request_id: request_id.clone(),
                data: event.data,
            };
            permit.send(Ok(sse_event(envelope)));
            if event_type == TERMINAL_EVENT_TYPE {
                return;
            }
        }
    });
    rx
}

#[allow(clippy::too_many_arguments)]
async fn durable_or_control_close(
    pool: &Pool,
    claims: &AccessClaims,
    session_id: Uuid,
    cited_document_ids: &[Uuid],
    request_id: &str,
    reason: &'static str,
    status: AskStreamStatus,
    tx: &tokio::sync::mpsc::Sender<Result<axum::response::sse::Event, std::convert::Infallible>>,
) -> Result<(), ()> {
    // Prefer durable terminal via sequence allocator when principal can open a txn.
    if let Ok(ctx) = stream_auth::revalidate_ask_stream(pool, claims, &[]).await {
        let cited = cited_document_ids.to_vec();
        let family_id = Uuid::parse_str(&claims.sid).ok();
        let durable = with_org_txn(pool, &ctx, {
            let ctx = ctx.clone();
            move |txn| {
                Box::pin(async move {
                    ask_streams::close_with_terminal(
                        txn, &ctx, family_id, session_id, status, reason, &cited,
                    )
                    .await
                })
            }
        })
        .await;
        if let Ok(Some(event)) = durable {
            let event_reason = event
                .data
                .get("reason")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if event_reason == reason {
                let envelope = SseEnvelope {
                    version: SSE_ENVELOPE_VERSION,
                    sequence: event.sequence_no as u64,
                    event: event.event_type,
                    request_id: request_id.into(),
                    data: event.data,
                };
                let _ = send_envelope(tx, envelope).await;
                return Ok(());
            }
            // Session already had an unrelated terminal (e.g. completed). Authz
            // revoke must still surface via a control frame without inventing ids.
        }
    }
    send_control_closed(tx, request_id, reason).await
}

/// Control frame without SSE id — must not advance client Last-Event-ID cursor.
async fn send_control_closed(
    tx: &tokio::sync::mpsc::Sender<Result<axum::response::sse::Event, std::convert::Infallible>>,
    request_id: &str,
    reason: &str,
) -> Result<(), ()> {
    let data = serde_json::json!({
        "version": SSE_ENVELOPE_VERSION,
        "event": "stream.closed",
        "requestId": request_id,
        "data": { "reason": reason },
        "control": true,
    });
    let event = axum::response::sse::Event::default()
        .event("stream.closed")
        .data(data.to_string());
    match tokio::time::timeout(Duration::from_millis(200), tx.send(Ok(event))).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}

#[derive(Debug)]
enum SendFail {
    SlowClient,
    Disconnected,
}

async fn send_envelope(
    tx: &tokio::sync::mpsc::Sender<Result<axum::response::sse::Event, std::convert::Infallible>>,
    envelope: SseEnvelope,
) -> Result<(), SendFail> {
    match tokio::time::timeout(ASK_STREAM_SEND_TIMEOUT, tx.send(Ok(sse_event(envelope)))).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(SendFail::Disconnected),
        Err(_) => Err(SendFail::SlowClient),
    }
}

fn sse_event(envelope: SseEnvelope) -> axum::response::sse::Event {
    let data = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".into());
    axum::response::sse::Event::default()
        .id(envelope.sequence.to_string())
        .event(envelope.event)
        .data(data)
}

pub fn keep_alive() -> axum::response::sse::KeepAlive {
    axum::response::sse::KeepAlive::new()
        .interval(HEARTBEAT_INTERVAL)
        .text("heartbeat")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn slow_client_reserve_times_out_with_stable_reason() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<
            Result<axum::response::sse::Event, std::convert::Infallible>,
        >(ASK_STREAM_CHANNEL_CAP);
        let first = SseEnvelope {
            version: SSE_ENVELOPE_VERSION,
            sequence: 1,
            event: "ask.token".into(),
            request_id: "req-slow".into(),
            data: json!({ "text": "a" }),
        };
        let permit = tx.reserve().await.expect("reserve first slot");
        permit.send(Ok(sse_event(first)));
        // Cap=1 full: next reserve waits on the client (no DB locks held).
        let err = tokio::time::timeout(ASK_STREAM_SEND_TIMEOUT, tx.reserve()).await;
        assert!(err.is_err(), "reserve must time out for slow client");
        assert_eq!(StreamAuthError::SendTimeout.close_reason(), "send_timeout");
    }

    #[test]
    fn retention_snapshot_omits_question_and_answer_keys() {
        let pinned = json!({
            "embeddingMode": "fts_only",
            "hitIds": [],
            "citationIds": [],
            "versionMode": "current",
            "warningCount": 0,
            "extractiveChars": 12,
            "questionSha256": "abc",
        });
        let text = pinned.to_string();
        assert!(!text.contains("question\":"));
        assert!(!text.contains("extractiveAnswer"));
        assert!(!text.contains("\"body\""));
        assert!(text.contains("questionSha256"));
    }
}
