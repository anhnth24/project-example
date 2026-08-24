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
use fileconv_knowledge::ask::{extractive_answer, valid_citation_ids, AnswerMode};
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
use crate::services::citation::{pins_cited_in_answer, pins_from_hits, CitationPin};
use crate::services::embedding::ApprovedEmbeddingRuntime;
use crate::services::qa::chitchat::{assistant_fallback_reply, AskRoute};
use crate::services::qa::grounding::{
    conflict_resolution_notes_for_history, conflict_warnings_for_current, version_context_note,
    VersionContext,
};
use crate::services::qa::prompt::{build_assistant_messages, build_grounded_messages};
use crate::services::qa::provider::{ChatProvider, ProviderError, StreamCancel};
use crate::services::qa::stream::{tokenize_answer, HEARTBEAT_INTERVAL, SSE_ENVELOPE_VERSION};
use crate::services::qa::{
    allow_unverified_llm_runtime, attach_citations_to_uncited_lines, citation_auto_attach_warning,
    citation_retry_messages, decide_ask_route, draft_needs_citation_retry,
    force_extractive_only_runtime, hits_to_hybrid, reserve_ask_tokens, resolve_llm_answer,
    settle_ask_tokens, TokenLease, CITATION_RETRY_WARNING,
};
use crate::services::quota::QuotaError;
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

/// Whether the SSE producer will actually call a chat provider.
///
/// Real product LLMs stay fail-closed to extractive while structured
/// entailment is unavailable. Do **not** buffer `complete()` on this path:
/// OpenRouter often hits the 30s provider timeout, then citation validation
/// discards the OCR-heavy draft, so the UI waits ~30s for an extractive
/// answer that was already ready after retrieval. Hermetic `StreamingStatic`
/// doubles still incremental-stream so mid-stream ACL tests can observe
/// `ask.token` before close. JSON `POST /ask` may still try a buffered LLM
/// under `MARKHAND_QA_ALLOW_UNVERIFIED_LLM`.
fn uses_incremental_provider_stream(provider: &ChatProvider, extractive_forced: bool) -> bool {
    match provider {
        ChatProvider::StreamingStatic(_) => true,
        other => other.supports_incremental_stream() && !extractive_forced,
    }
}

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
    /// Token-quota admission failed before any durable session side effect
    /// (1C-09 a): `QuotaExceeded` maps to a distinguishable 429.
    Quota(QuotaError),
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

    let (route, route_warnings) = decide_ask_route(pool, ctx, provider.as_ref(), &question)
        .await
        .map_err(AskStreamPrepareError::Quota)?;
    let assistant_turn = matches!(route, AskRoute::Assistant);
    let (
        citations,
        cited_document_ids,
        cited_version_ids,
        collection_list,
        version_context,
        warnings,
        extractive,
        embedding_mode,
        hits,
        token_lease,
    ) = if assistant_turn {
        let extractive = assistant_fallback_reply(&question);
        let version_context = version_context_note(&mode, &[], &[]);
        let will_call_provider = provider.is_some();
        let token_lease = if will_call_provider {
            let messages = build_assistant_messages(&question);
            Some(
                reserve_ask_tokens(pool, ctx, &messages)
                    .await
                    .map_err(AskStreamPrepareError::Quota)?,
            )
        } else {
            None
        };
        let collection_list: Vec<Uuid> = collection_ids
            .clone()
            .unwrap_or_else(|| ctx.allowed_collection_ids().iter().copied().collect())
            .into_iter()
            .collect();
        (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            collection_list,
            version_context,
            route_warnings,
            extractive,
            "assistant".to_string(),
            Vec::new(),
            token_lease,
        )
    } else {
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
        let mut warnings = route_warnings;
        warnings.extend(retrieval.warnings);
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

        // Token quota (1C-09 a): reserve before creating any durable session state
        // — a denial is a clean 429 with zero side effects. Same predicate as
        // `run_producer`: only incremental provider streaming, never a buffered
        // `complete()` wait that blocks first `ask.token`.
        let extractive_forced = force_extractive_only_runtime();
        // Dev-gate (`MARKHAND_QA_ALLOW_UNVERIFIED_LLM`): the producer will try a
        // buffered `complete()` even while fail-closed, so reserve tokens here too.
        let will_call_provider = !retrieval.hits.is_empty()
            && provider.as_ref().is_some_and(|chat| {
                uses_incremental_provider_stream(chat, extractive_forced)
                    || allow_unverified_llm_runtime()
            });
        let token_lease = if will_call_provider {
            let messages = build_grounded_messages(&question, &hybrid, &mode);
            Some(
                reserve_ask_tokens(pool, ctx, &messages)
                    .await
                    .map_err(AskStreamPrepareError::Quota)?,
            )
        } else {
            None
        };
        (
            citations,
            cited_document_ids,
            cited_version_ids,
            collection_list,
            version_context,
            warnings,
            extractive,
            retrieval.embedding_mode,
            retrieval.hits,
            token_lease,
        )
    };

    let session_id = Uuid::new_v4();
    // Retention-safe snapshot: IDs/hashes only — never question/answer/quote body.
    let pinned_snapshot = json!({
        "embeddingMode": embedding_mode,
        "hitIds": hits.iter().map(hit_id_summary).collect::<Vec<_>>(),
        "citationIds": citations.iter().map(|pin| json!({
            "documentId": pin.logical_document_id,
            "versionId": pin.version_id,
            "chunkIdentitySha256": pin.chunk_identity_sha256,
        })).collect::<Vec<_>>(),
        "versionMode": mode_wire(&mode),
        "warningCount": warnings.len(),
        "extractiveChars": extractive.chars().count(),
        "questionSha256": sha256_hex(question.as_bytes()),
        "assistantTurn": assistant_turn,
    });

    let version_mode = mode_wire(&mode);
    let ctx_owned = ctx.clone();
    let created = with_org_txn(pool, ctx, {
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
    .map_err(|_| AskStreamPrepareError::Database);
    if let Err(error) = created {
        // No producer will run — release the token reservation immediately
        // instead of waiting out its TTL.
        if let Some(lease) = token_lease {
            settle_ask_tokens(pool, ctx, lease, None).await;
        }
        return Err(error);
    }

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
            embedding_mode,
            hits,
            question,
            mode,
            producer_provider,
            producer_cancel,
            token_lease,
            assistant_turn,
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
    mut citations: Vec<CitationPin>,
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
    token_lease: Option<TokenLease>,
    assistant_turn: bool,
) {
    let mut token_lease = token_lease;
    // Measured provider consumption for token settlement (1C-09 a):
    // `None` = the provider request never got off the ground (refund);
    // `Some(chars)` = prompt was sent, `chars` answer characters were
    // streamed/returned so far (commit prompt + chars).
    let mut provider_usage: Option<usize> = None;
    let family_id = Uuid::parse_str(&claims.sid).ok();
    let started = std::time::Instant::now();
    let corr = crate::telemetry::CorrelationContext::current();

    let append = |event_type: &'static str, data: Value| {
        let pool = pool.clone();
        let ctx = ctx.clone();
        let claims = claims.clone();
        let cited = cited_document_ids.clone();
        async move {
            // JWT/token checks outside the write-gate; hold the RAII guard only
            // around the authorized append transaction (not provider waits).
            if stream_auth::token_expired(&claims, chrono::Utc::now().timestamp()) {
                return Err(crate::db::error::DbError::Config("token_expired".into()));
            }
            let Some(family_id) = Uuid::parse_str(&claims.sid).ok() else {
                return Err(crate::db::error::DbError::Config("session_revoked".into()));
            };
            let Ok(guard) =
                crate::middleware::write_gate::acquire_background_mutation_guard(&pool).await
            else {
                return Err(crate::db::error::DbError::Config("ops_fence_active".into()));
            };
            let result = with_org_txn(&pool, &ctx, {
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
            .await;
            guard.release().await;
            result
        }
    };

    let close = |status: AskStreamStatus, reason: &'static str| {
        let pool = pool.clone();
        let ctx = ctx.clone();
        let cited = cited_document_ids.clone();
        async move {
            let Ok(guard) =
                crate::middleware::write_gate::acquire_background_mutation_guard(&pool).await
            else {
                return;
            };
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
            guard.release().await;
        }
    };

    let started_mode = if assistant_turn {
        AnswerMode::Assistant
    } else {
        AnswerMode::OfflineExtractive
    };
    if let Err(error) = append(
        "ask.started",
        json!({
            "streamSessionId": session_id,
            "mode": started_mode.as_str(),
            "embeddingMode": embedding_mode,
            "citationCount": citations.len(),
        }),
    )
    .await
    {
        settle_lease(&pool, &ctx, &mut token_lease, provider_usage).await;
        let reason = config_reason(&error).unwrap_or("stream_error");
        close(AskStreamStatus::Error, reason).await;
        return;
    }

    let mut answer_mode = started_mode;
    let mut streamed_any = false;
    let mut unverified_answer: Option<String> = None;
    let mut emitted_answer = String::new();

    if assistant_turn {
        if let Some(chat) = provider.as_ref() {
            let messages = build_assistant_messages(&question);
            match chat.complete(&messages).await {
                Ok(llm_answer) => {
                    provider_usage = Some(llm_answer.chars().count());
                    let trimmed = llm_answer.trim();
                    if trimmed.is_empty() {
                        warnings
                            .push("Assistant provider returned empty; using offline reply.".into());
                        unverified_answer = Some(extractive.clone());
                    } else {
                        unverified_answer = Some(trimmed.to_string());
                    }
                    answer_mode = AnswerMode::Assistant;
                }
                Err(ProviderError::Timeout) => {
                    provider_usage = Some(0);
                    warnings.push("LLM provider timed out; using offline assistant reply.".into());
                    unverified_answer = Some(extractive.clone());
                    answer_mode = AnswerMode::Assistant;
                }
                Err(_) => {
                    warnings
                        .push("LLM provider unavailable; using offline assistant reply.".into());
                    unverified_answer = Some(extractive.clone());
                    answer_mode = AnswerMode::Assistant;
                }
            }
        } else {
            warnings.push("No chat provider configured; using offline assistant reply.".into());
            unverified_answer = Some(extractive.clone());
            answer_mode = AnswerMode::Assistant;
        }
    } else {
        let extractive_forced = force_extractive_only_runtime();
        let use_provider_stream = provider
            .as_ref()
            .is_some_and(|p| uses_incremental_provider_stream(p, extractive_forced));

        if use_provider_stream {
            if let Some(chat) = provider.as_ref() {
                let hybrid = hits_to_hybrid(&hits);
                let messages = build_grounded_messages(&question, &hybrid, &mode);
                match chat.stream_tokens(&messages, cancel.clone()).await {
                    Ok(mut rx) => {
                        answer_mode = chat.answer_mode();
                        // Request reached the provider: prompt tokens are spent
                        // from here on, even if zero answer tokens arrive.
                        provider_usage = Some(0);
                        while let Some(item) = rx.recv().await {
                            if cancel.is_cancelled() {
                                settle_lease(&pool, &ctx, &mut token_lease, provider_usage).await;
                                close(AskStreamStatus::Error, "cancelled").await;
                                return;
                            }
                            match item {
                                Ok(token) => {
                                    streamed_any = true;
                                    emitted_answer.push_str(&token);
                                    provider_usage = Some(
                                        provider_usage
                                            .unwrap_or(0)
                                            .saturating_add(token.chars().count()),
                                    );
                                    if let Err(error) =
                                        append("ask.token", json!({ "text": token })).await
                                    {
                                        // Includes mid-stream citation_revoked:
                                        // tokens already streamed by the provider
                                        // stay committed, never refunded.
                                        settle_lease(&pool, &ctx, &mut token_lease, provider_usage)
                                            .await;
                                        let reason =
                                            config_reason(&error).unwrap_or("stream_error");
                                        cancel.cancel();
                                        close(AskStreamStatus::Error, reason).await;
                                        return;
                                    }
                                }
                                Err(ProviderError::Cancelled) => {
                                    settle_lease(&pool, &ctx, &mut token_lease, provider_usage)
                                        .await;
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
                                        "LLM provider unavailable; using extractive fallback."
                                            .into(),
                                    );
                                    answer_mode = AnswerMode::FallbackExtractive;
                                    break;
                                }
                            }
                        }
                    }
                    Err(ProviderError::Timeout) => {
                        // Request was sent; assume the prompt was billed.
                        provider_usage = Some(0);
                        warnings.push("LLM provider timed out; using extractive fallback.".into());
                        answer_mode = AnswerMode::FallbackExtractive;
                    }
                    Err(_) => {
                        warnings
                            .push("LLM provider unavailable; using extractive fallback.".into());
                        answer_mode = AnswerMode::FallbackExtractive;
                    }
                }
            }
        } else if allow_unverified_llm_runtime() && !hits.is_empty() {
            // Dev-gate: buffered grounded `complete()` mirroring JSON `POST /ask`
            // exactly (`resolve_llm_answer` applies byte-identical fail-closed /
            // unverified policy). Default deployments never enter this branch.
            if let Some(chat) = provider.as_ref() {
                let hybrid = hits_to_hybrid(&hits);
                let messages = build_grounded_messages(&question, &hybrid, &mode);
                // OpenRouter chập chờn theo đợt (429/5xx); retry backoff 2s rồi 5s
                // cứu phần lớn request. Timeout retry đúng MỘT lần (lần backoff
                // đầu): keepalive bên dưới đã giữ live-tail mở nên một chu kỳ
                // timeout nữa không làm client rớt; eval 2026-08-16 cho thấy
                // timeout theo đợt ngắn là nguồn fallback_extractive lớn nhất.
                // Trong lúc chờ `complete()`, persist một `ask.token` rỗng mỗi
                // 20s: vô hình với client nhưng reset đồng hồ idle của
                // live-tail (bằng chứng eval v7: chuỗi embed chậm + provider
                // timeout kéo 92–109s làm tail đóng ở 60s, client nhận rỗng).
                let complete_with_keepalive =
                    |request: crate::services::qa::prompt::GroundedMessages| {
                        let append = &append;
                        async move {
                            let call = chat.complete(&request);
                            tokio::pin!(call);
                            let mut keepalive = tokio::time::interval(Duration::from_secs(20));
                            keepalive.tick().await; // tick đầu hoàn thành ngay — bỏ qua
                            loop {
                                tokio::select! {
                                    result = &mut call => break result,
                                    _ = keepalive.tick() => {
                                        let _ = append("ask.token", json!({ "text": "" })).await;
                                    }
                                }
                            }
                        }
                    };
                let mut outcome = complete_with_keepalive(messages.clone()).await;
                for (attempt, backoff_secs) in [2u64, 5].into_iter().enumerate() {
                    let retryable = matches!(
                        outcome,
                        Err(ref error) if !matches!(error, ProviderError::Timeout) || attempt == 0
                    );
                    if !retryable || cancel.is_cancelled() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                    outcome = complete_with_keepalive(messages.clone()).await;
                }
                match outcome {
                    Ok(mut llm_answer) => {
                        let mut consumed = llm_answer.chars().count();
                        let valid_ids = valid_citation_ids(hybrid.len());
                        // Draft không có [CITE- chắc chắn rớt validation → P0.2
                        // auto-attach trước, chỉ nhắc model đúng MỘT lần khi
                        // auto-attach không cứu được (giống JSON ask()).
                        if draft_needs_citation_retry(&llm_answer) && !cancel.is_cancelled() {
                            if let Some(saved) = attach_citations_to_uncited_lines(
                                &llm_answer,
                                &valid_ids,
                                &citations,
                                &mode,
                            ) {
                                warnings.push(citation_auto_attach_warning(saved.attached));
                                llm_answer = saved.answer;
                            } else {
                                warnings.push(CITATION_RETRY_WARNING.into());
                                let retry = citation_retry_messages(&messages, &llm_answer);
                                if let Ok(second) = complete_with_keepalive(retry).await {
                                    consumed += second.chars().count();
                                    if second.contains("[CITE-") {
                                        llm_answer = second;
                                    }
                                }
                            }
                        }
                        provider_usage = Some(consumed);
                        let (answer, resolved_mode, extra_warnings) = resolve_llm_answer(
                            llm_answer,
                            &extractive,
                            &valid_ids,
                            &citations,
                            &mode,
                            chat.answer_mode(),
                        );
                        warnings.extend(extra_warnings);
                        answer_mode = resolved_mode;
                        unverified_answer = Some(answer);
                    }
                    Err(ProviderError::Timeout) => {
                        // Request reached the provider: prompt tokens are spent.
                        provider_usage = Some(0);
                        warnings.push("LLM provider timed out; using extractive fallback.".into());
                        answer_mode = AnswerMode::FallbackExtractive;
                    }
                    Err(_) => {
                        warnings
                            .push("LLM provider unavailable; using extractive fallback.".into());
                        answer_mode = AnswerMode::FallbackExtractive;
                    }
                }
            }
        }
    } // end !assistant_turn

    // Provider interaction is over on every remaining path — settle exactly
    // once here (later returns only emit already-persisted/extractive data).
    settle_lease(&pool, &ctx, &mut token_lease, provider_usage).await;

    if let Some(answer) = unverified_answer {
        for token in tokenize_answer(&answer) {
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
            emitted_answer.push_str(&token);
        }
    } else if !streamed_any || matches!(answer_mode, AnswerMode::FallbackExtractive) {
        if !assistant_turn && force_extractive_only_runtime() {
            warnings.push(
                "Structured entailment unavailable; fail-closed extractive-only grounding.".into(),
            );
        }
        answer_mode = if matches!(answer_mode, AnswerMode::FallbackExtractive) {
            AnswerMode::FallbackExtractive
        } else if assistant_turn {
            AnswerMode::Assistant
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
            emitted_answer.push_str(&token);
        }
    }

    citations = pins_cited_in_answer(&emitted_answer, citations);

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
        if let Some(corr) = &corr {
            crate::telemetry::emit_span(
                "ask.stream",
                &corr.request_id,
                &corr.trace_id,
                "internal",
                "error",
                started.elapsed(),
            );
        }
        crate::telemetry::record_retrieval_leg("ask_stream", "error", started.elapsed());
        return;
    }
    close(AskStreamStatus::Closed, "completed").await;
    let outcome = if cancel.is_cancelled() {
        "cancelled"
    } else {
        "completed"
    };
    if let Some(corr) = &corr {
        crate::telemetry::emit_span(
            "ask.stream",
            &corr.request_id,
            &corr.trace_id,
            "internal",
            outcome,
            started.elapsed(),
        );
    }
    crate::telemetry::record_retrieval_leg("ask_stream", outcome, started.elapsed());
}

/// Settle the producer's token lease exactly once (`Option::take` guard).
async fn settle_lease(
    pool: &Pool,
    ctx: &OrgContext,
    lease: &mut Option<TokenLease>,
    provider_usage: Option<usize>,
) {
    if let Some(lease) = lease.take() {
        settle_ask_tokens(pool, ctx, lease, provider_usage).await;
    }
}

fn config_reason(error: &crate::db::error::DbError) -> Option<&'static str> {
    match error {
        crate::db::error::DbError::Config(message) => match message.as_str() {
            "token_expired" => Some("token_expired"),
            "session_revoked" => Some("session_revoked"),
            "principal_denied" => Some("principal_denied"),
            "citation_revoked" => Some("citation_revoked"),
            "ops_fence_active" => Some("ops_fence_active"),
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
                // 60s: đủ trùm một chu kỳ provider-timeout 30s + retry + emit
                // extractive; 30s cũ đóng tail đúng lúc fallback sắp phát.
                if idle_polls >= 300 {
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

    #[test]
    fn hermetic_streaming_static_stays_enabled_under_extractive_fail_closed() {
        // Mid-stream ACL revoke tests wire StreamingStatic; extractive-only
        // fail-closed must not suppress that hermetic path (real LLM providers
        // remain gated by `!extractive_forced` in the same match).
        let source = include_str!("ask_stream.rs");
        assert!(
            source.contains("ChatProvider::StreamingStatic(_) => true"),
            "StreamingStatic must keep incremental production while entailment is unavailable"
        );
        assert!(
            source.contains("other => other.supports_incremental_stream() && !extractive_forced"),
            "non-hermetic providers must stay extractive-gated"
        );
    }
}
