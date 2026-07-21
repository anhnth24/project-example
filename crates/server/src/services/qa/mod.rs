//! Grounded Q&A with version-aware citations, streaming, and extractive fallback (P1B-R03).
//!
//! Bound to fresh [`OrgContext`] + [`RetrievalProvenance`]. Citations are built via
//! the R02 trusted-Markdown path. Routes/SSE remain R05.

pub mod authz_fence;
pub mod grounding;
pub mod prompt;
pub mod provider;
pub mod stream;

use std::collections::BTreeSet;
use std::time::Duration;

use bytes::Bytes;
use chrono::Utc;
use deadpool_postgres::Pool;
use serde_json::{json, Value as JsonValue};
use thiserror::Error;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{require_permission, resolve_org_context_in_txn};
use crate::db::authz_epoch;
use crate::db::authz_lock::LockPool;
use crate::db::pool::{with_org_txn, with_org_txn_serializable};
use crate::db::search::{self, dedup_sorted_uuids, VersionTimelineRow};
use crate::services::authz_mutation;
use crate::services::citation::{
    resolve_citations_batched, CitationError, CitationResolveRequest, StableCitation,
};
use crate::services::qa::grounding::{
    append_server_notes, build_version_context, conflict_messages, ensure_versions_represented,
    extractive_answer, extractive_structured_claims, filter_compare_passages,
    page_history_timeline, passages_for_mode, render_answer_from_claims, revalidate_final_answer,
    typed_delta_from_pair_row, typed_numeric_delta_for_versions, validate_grounded_answer,
    validate_structured_claims, ConflictWarning, FinalAnswerCheck, GroundingError, HistoryPageMeta,
    QaConflict, ServerNote, ServerNoteKind, TypedVersionClaim, VersionContext,
};
use crate::services::qa::prompt::{
    grounded_user_prompt, system_policy_is_separated, PromptPassage, GROUNDED_SYSTEM_POLICY,
};
use crate::services::qa::provider::{
    parse_grounded_payload, ChatCompletionRequest, ProviderError, QaChatProvider, QaProviderConfig,
    MAX_RESPONSE_BYTES,
};
use crate::services::qa::stream::{
    run_bounded_stream, spawn_epoch_watch, tokenize_for_stream, ProtectedStreamReceiver,
};
use crate::services::retrieval::{
    resolve_scope, RetrievalHit, RetrievalProvenance, RetrievalResponse, VersionMode,
    PERMISSION_QA_HISTORY, PERMISSION_QA_QUERY,
};
use crate::storage::blob::BlobStore;

pub use authz_fence::{AuthzEpochFence, CloseKind, CloseKind as AuthzCloseKind};
pub use grounding::{cite_id, GroundingPassage, GroundingPassage as QaPassage, StructuredClaim};
pub use prompt::GROUNDED_SYSTEM_POLICY as SYSTEM_POLICY;
pub use provider::{
    canonicalize_base_url, ConfiguredProvider, GlmCompatibleProvider, HangingProvider,
    ProviderAuth, ScriptedProvider, ENV_QA_ALLOWED_HOSTS, ENV_QA_ALLOW_LOCAL, ENV_QA_ALLOW_NO_AUTH,
    ENV_QA_API_KEY, ENV_QA_BASE_URL, ENV_QA_MODEL, ENV_QA_PROVIDER, ENV_QA_TIMEOUT_MS,
};
pub use stream::{
    collect_sse_token_text, drive_body_to_end, spawn_db_authz_watch as spawn_qa_db_authz_watch,
    spawn_epoch_watch as spawn_qa_epoch_watch, AuthzProbeResult, AuthzWatch,
    AuthzWatch as QaAuthzWatch, DeliveryLockContext, EpochWatchResult, GuardedJsonBody,
    GuardedSseBody, StreamBounds, StreamBounds as QaStreamBounds, StreamCancel,
    StreamCancel as QaStreamCancel, StreamCloseReason, StreamError, StreamEvent,
    StreamEvent as QaStreamEvent, DEFAULT_RESPONSE_LIFETIME, DEFAULT_STALL_WATCHDOG,
};

pub const MAX_QUESTION_CHARS: usize = 4_000;
pub const MAX_PASSAGES: usize = 32;
pub const MAX_PASSAGE_CHARS: usize = 8_000;
pub const MAX_PROMPT_BYTES: usize = 128 * 1024;
pub const MAX_HISTORY_VERSIONS: usize = 32;
pub const MAX_TOTAL_PASSAGE_BYTES: usize = 256 * 1024;
pub const MAX_CONFLICTS: i64 = 32;
pub const STALE_RETRIEVAL_MAX_AGE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerMode {
    OfflineExtractive,
    FallbackExtractive,
    ProviderLlm,
}

impl AnswerMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OfflineExtractive => "offline_extractive",
            Self::FallbackExtractive => "fallback_extractive",
            Self::ProviderLlm => "provider_llm",
        }
    }
}

/// Q&A request. `request_id` is always server-generated when using [`answer_question`].
#[derive(Clone, PartialEq)]
pub struct QaRequest {
    pub question: String,
    pub mode: VersionMode,
    pub use_llm: bool,
    pub collection_ids: Option<BTreeSet<Uuid>>,
}

impl std::fmt::Debug for QaRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QaRequest")
            .field("question", &"[REDACTED]")
            .field("mode", &self.mode)
            .field("use_llm", &self.use_llm)
            .field("collection_ids", &self.collection_ids)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
pub struct QaAnswer {
    pub answer: String,
    pub citations: Vec<StableCitation>,
    pub mode: AnswerMode,
    pub grounded: bool,
    pub warnings: Vec<String>,
    pub version_context: VersionContext,
    pub conflict_warnings: Vec<ConflictWarning>,
    pub audit: QaAuditMetadata,
}

impl std::fmt::Debug for QaAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QaAnswer")
            .field("answer", &"[REDACTED]")
            .field("citations", &self.citations.len())
            .field("mode", &self.mode)
            .field("grounded", &self.grounded)
            .field("warnings_count", &self.warnings.len())
            .field("version_context", &self.version_context.mode)
            .field("conflict_warnings", &self.conflict_warnings.len())
            .field("audit", &self.audit)
            .finish()
    }
}

/// Pins validated under the final delivery guard (carried into stream handoff).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDeliverySnapshot {
    pub captured_epoch: u64,
    pub document_ids: Vec<Uuid>,
    pub version_ids: Vec<Uuid>,
    pub collection_ids: Vec<Uuid>,
    pub current_version_ids: Vec<Uuid>,
    pub require_history: bool,
    pub mode_label: &'static str,
}

/// Non-stream live response: concrete [`GuardedJsonBody`] via [`IntoResponse`] only.
///
/// Guard is acquired + exact-revalidated before construction and released only when
/// the HTTP body ends or is dropped. No plaintext materialization API.
///
/// **Emission guarantee (HTTP-honest):** after revoke, the server stops generating
/// and enqueueing new application frames. Bytes already handed to Hyper/the kernel
/// cannot be recalled; at most one small already-encoded frame may still be in
/// flight on the transport (bounded tail).
pub struct GuardedQaResponse {
    body: GuardedJsonBody,
    /// Metadata for route headers / stream handoff (answer bytes already in body).
    meta: GuardedQaMeta,
}

#[derive(Debug, Clone)]
struct GuardedQaMeta {
    citations: Vec<StableCitation>,
    warnings: Vec<String>,
    conflict_warnings: Vec<ConflictWarning>,
    version_context: VersionContext,
    audit: QaAuditMetadata,
    grounded: bool,
    mode: AnswerMode,
    /// Full answer retained only for stream handoff (not a public collect path).
    answer_for_stream: Option<QaAnswer>,
    /// Epoch/mode/pins validated under the final delivery guard.
    snapshot: ValidatedDeliverySnapshot,
}

impl std::fmt::Debug for GuardedQaResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedQaResponse")
            .field("grounded", &self.meta.grounded)
            .field("mode", &self.meta.mode)
            .finish_non_exhaustive()
    }
}

impl GuardedQaResponse {
    pub fn citations(&self) -> &[crate::services::citation::StableCitation] {
        &self.meta.citations
    }
    pub fn warnings(&self) -> &[String] {
        &self.meta.warnings
    }
    pub fn conflict_warnings(&self) -> &[ConflictWarning] {
        &self.meta.conflict_warnings
    }
    pub fn version_context(&self) -> &VersionContext {
        &self.meta.version_context
    }
    pub fn audit(&self) -> &QaAuditMetadata {
        &self.meta.audit
    }
    pub fn grounded(&self) -> bool {
        self.meta.grounded
    }
    pub fn mode(&self) -> AnswerMode {
        self.meta.mode
    }

    /// End the HTTP response lifetime: unlock the session delivery guard.
    pub async fn finish(self) {
        self.body.finish().await;
    }

    pub fn delivery_snapshot(&self) -> &ValidatedDeliverySnapshot {
        &self.meta.snapshot
    }

    fn from_answer(
        answer: QaAnswer,
        guard: crate::db::authz_lock::DeliveryGuard,
        snapshot: ValidatedDeliverySnapshot,
    ) -> Result<Self, QaError> {
        let bytes = Bytes::from(
            serde_json::to_vec(&qa_answer_envelope_json(&answer)).map_err(|_| QaError::Database)?,
        );
        let meta = GuardedQaMeta {
            citations: answer.citations.clone(),
            warnings: answer.warnings.clone(),
            conflict_warnings: answer.conflict_warnings.clone(),
            version_context: answer.version_context.clone(),
            audit: answer.audit.clone(),
            grounded: answer.grounded,
            mode: answer.mode,
            answer_for_stream: Some(answer),
            snapshot,
        };
        Ok(Self {
            body: GuardedJsonBody::new(bytes, guard, DEFAULT_RESPONSE_LIFETIME),
            meta,
        })
    }

    /// Stream handoff: take answer + validated pins; abandon JSON body guard
    /// (stream re-acquires). Rejects if epoch/current pointer already drifted.
    pub(crate) async fn into_answer_for_stream(
        mut self,
        pool: &Pool,
        ctx: &OrgContext,
    ) -> Result<(QaAnswer, ValidatedDeliverySnapshot), QaError> {
        let snapshot = self.meta.snapshot.clone();
        // Reject handoff when epoch or current pointer changed since final guard.
        let org_id = ctx.org_id();
        let user_id = ctx.user_id();
        let docs = snapshot.document_ids.clone();
        let versions = snapshot.version_ids.clone();
        let colls = snapshot.collection_ids.clone();
        let require_history = snapshot.require_history;
        let captured = snapshot.captured_epoch;
        let expected_current = snapshot.current_version_ids.clone();
        let mode_label = snapshot.mode_label;
        with_org_txn(pool, ctx, {
            move |txn| {
                Box::pin(async move {
                    let probe = search::probe_stream_authz_exact(
                        txn,
                        org_id,
                        user_id,
                        &docs,
                        &versions,
                        &colls,
                        require_history,
                    )
                    .await?;
                    if !matches!(probe, search::StreamAuthzProbe::Allow) {
                        return Err(crate::db::error::DbError::Config(
                            "qa handoff auth denied".into(),
                        ));
                    }
                    let snap =
                        authz_epoch::read_epoch_snapshot(txn, org_id, user_id, &docs).await?;
                    if snap.composite() != captured {
                        return Err(crate::db::error::DbError::Config(
                            "qa handoff epoch race".into(),
                        ));
                    }
                    if !expected_current.is_empty()
                        && matches!(mode_label, "current" | "as_of" | "history")
                    {
                        let live = search::load_current_published_version_ids(
                            txn,
                            &OrgContext::try_new(
                                org_id,
                                user_id,
                                [PERMISSION_QA_QUERY],
                                colls.clone(),
                            )
                            .map_err(|e| crate::db::error::DbError::Config(e.to_string()))?,
                            &docs,
                            &colls,
                        )
                        .await?;
                        let live_set: BTreeSet<Uuid> = live.into_iter().collect();
                        let expect_set: BTreeSet<Uuid> = expected_current.iter().copied().collect();
                        if live_set != expect_set {
                            return Err(crate::db::error::DbError::Config(
                                "qa handoff pointer race".into(),
                            ));
                        }
                    }
                    Ok(())
                })
            }
        })
        .await
        .map_err(|err| {
            let msg = format!("{err}");
            if msg.contains("pointer") || msg.contains("epoch") {
                QaError::StaleRetrieval
            } else {
                QaError::PermissionDenied
            }
        })?;
        drop(self.body);
        let answer = self
            .meta
            .answer_for_stream
            .take()
            .expect("answer present for stream handoff");
        Ok((answer, snapshot))
    }
}

fn citation_json(c: &StableCitation) -> JsonValue {
    json!({
        "org_id": c.org_id,
        "logical_document_id": c.logical_document_id,
        "version_id": c.version_id,
        "version_number": c.version_number,
        "content_sha256": c.content_sha256,
        "chunk_id": c.chunk_id,
        "chunk_identity_sha256": c.chunk_identity_sha256,
        "page": c.page,
        "slide": c.slide,
        "sheet": c.sheet,
        "span_start": c.span_start,
        "span_end": c.span_end,
        "quote": c.quote,
        "effective_from": c.effective_from,
        "effective_to": c.effective_to,
        "is_current": c.is_current,
        "heading": c.heading,
    })
}

fn conflict_warning_json(w: &ConflictWarning) -> JsonValue {
    json!({
        "conflict_id": w.conflict_id,
        "status": w.status,
        "message": w.message,
        "pin_cite_ids": w.pin_cite_ids,
    })
}

fn version_context_json(v: &VersionContext) -> JsonValue {
    json!({
        "mode": v.mode,
        "current_version_ids": v.current_version_ids,
        "cited_version_ids": v.cited_version_ids,
        "change_note": v.change_note,
        "history": v.history.iter().map(|h| json!({
            "version_id": h.version_id,
            "version_number": h.version_number,
            "is_current": h.is_current,
            "effective_from": h.effective_from,
            "effective_to": h.effective_to,
            "content_sha256": h.content_sha256,
        })).collect::<Vec<_>>(),
        "history_page": v.history_page.as_ref().map(|p| json!({
            "truncated": p.truncated,
            "limit": p.limit,
            "returned": p.returned,
            "before_version_no": p.before_version_no,
            "next_before_version_no": p.next_before_version_no,
        })),
    })
}

/// Full stable JSON envelope (pins + version_context + conflicts).
pub fn qa_answer_envelope_json(answer: &QaAnswer) -> JsonValue {
    json!({
        "answer": answer.answer,
        "grounded": answer.grounded,
        "mode": answer.mode.as_str(),
        "citations": answer.citations.iter().map(citation_json).collect::<Vec<_>>(),
        "warnings": answer.warnings,
        "conflict_warnings": answer.conflict_warnings.iter().map(conflict_warning_json).collect::<Vec<_>>(),
        "version_context": version_context_json(&answer.version_context),
        "audit": {
            "action": answer.audit.action,
            "outcome": answer.audit.outcome,
            "answer_mode": answer.audit.answer_mode,
            "citation_count": answer.audit.citation_count,
            "conflict_warning_count": answer.audit.conflict_warning_count,
            "version_mode": answer.audit.version_mode,
            "provider_configured": answer.audit.provider_configured,
            "fallback_reason": answer.audit.fallback_reason,
            "request_id": answer.audit.request_id,
            "grounded": answer.audit.grounded,
        },
    })
}

/// Acquire final delivery guard and exact-recheck epoch/ACL/perms/mode pins.
async fn acquire_final_delivery_guard(
    lock_pool: &LockPool,
    pool: &Pool,
    ctx: &OrgContext,
    answer: &QaAnswer,
    collection_ids: &[Uuid],
) -> Result<
    (
        crate::db::authz_lock::DeliveryGuard,
        ValidatedDeliverySnapshot,
    ),
    QaError,
> {
    let document_ids: Vec<Uuid> = answer
        .citations
        .iter()
        .map(|c| c.logical_document_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let version_ids: Vec<Uuid> = answer
        .citations
        .iter()
        .map(|c| c.version_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let require_history = !matches!(answer.version_context.mode, "current");
    let current_version_ids = answer.version_context.current_version_ids.clone();
    let guard = crate::db::authz_lock::DeliveryGuard::acquire_shared(
        lock_pool,
        ctx.org_id(),
        ctx.user_id(),
        &document_ids,
        collection_ids,
    )
    .await
    .map_err(|_| QaError::PermissionDenied)?;
    let org_id = ctx.org_id();
    let user_id = ctx.user_id();
    let client = match guard.client() {
        Ok(c) => c,
        Err(_) => {
            guard.abandon().await;
            return Err(QaError::PermissionDenied);
        }
    };
    let probe = search::probe_stream_authz_exact(
        client,
        org_id,
        user_id,
        &document_ids,
        &version_ids,
        collection_ids,
        require_history,
    )
    .await;
    let probe = match probe {
        Ok(p) => p,
        Err(_) => {
            guard.abandon().await;
            return Err(QaError::PermissionDenied);
        }
    };
    if !matches!(probe, search::StreamAuthzProbe::Allow) {
        guard.abandon().await;
        return Err(QaError::PermissionDenied);
    }
    let snap =
        match authz_epoch::read_epoch_snapshot_on_client(client, org_id, user_id, &document_ids)
            .await
        {
            Ok(s) => s,
            Err(_) => {
                guard.abandon().await;
                return Err(QaError::PermissionDenied);
            }
        };
    // Mode pin: for current/as_of/history, published current pointers must still match.
    // Compare pins lineage versions via probe/version ids (not current-set equality).
    if !current_version_ids.is_empty()
        && matches!(answer.version_context.mode, "current" | "as_of" | "history")
    {
        let ctx_pin = ctx.clone();
        let docs = document_ids.clone();
        let colls = collection_ids.to_vec();
        let live = with_org_txn(pool, ctx, {
            let ctx_pin = ctx_pin.clone();
            move |txn| {
                Box::pin(async move {
                    search::load_current_published_version_ids(txn, &ctx_pin, &docs, &colls).await
                })
            }
        })
        .await;
        match live {
            Ok(live) => {
                let live_set: BTreeSet<Uuid> = live.into_iter().collect();
                let expect_set: BTreeSet<Uuid> = current_version_ids.iter().copied().collect();
                if live_set != expect_set {
                    guard.abandon().await;
                    return Err(QaError::StaleRetrieval);
                }
            }
            Err(_) => {
                guard.abandon().await;
                return Err(QaError::PermissionDenied);
            }
        }
    }
    let snapshot = ValidatedDeliverySnapshot {
        captured_epoch: snap.composite(),
        document_ids,
        version_ids,
        collection_ids: collection_ids.to_vec(),
        current_version_ids,
        require_history,
        mode_label: answer.version_context.mode,
    };
    Ok((guard, snapshot))
}

impl axum::response::IntoResponse for GuardedQaResponse {
    fn into_response(self) -> axum::response::Response {
        (
            [(
                axum::http::header::CONTENT_TYPE,
                "application/json; charset=utf-8",
            )],
            axum::body::Body::new(self.body),
        )
            .into_response()
    }
}

/// Protected streaming session: metadata + session-scoped SSE body parts.
pub struct ProtectedStreamSession {
    pub citations: Vec<StableCitation>,
    pub warnings: Vec<String>,
    pub conflict_warnings: Vec<ConflictWarning>,
    pub version_context: VersionContext,
    pub audit: QaAuditMetadata,
    pub grounded: bool,
    pub mode: AnswerMode,
    /// Full JSON envelope for the protected SSE `metadata` event (before tokens).
    metadata_json: Bytes,
    receiver: ProtectedStreamReceiver,
    session_guard: Option<crate::db::authz_lock::DeliveryGuard>,
}

impl ProtectedStreamSession {
    pub fn fence(&self) -> Option<&AuthzEpochFence> {
        self.receiver.fence()
    }

    pub fn into_sse_body(self) -> GuardedSseBody {
        let (events, fence, cancel) = self.receiver.into_parts();
        GuardedSseBody::new(
            events,
            self.session_guard,
            fence,
            Some(cancel),
            Some(self.metadata_json),
            DEFAULT_RESPONSE_LIFETIME,
            crate::services::qa::stream::DEFAULT_STALL_WATCHDOG,
        )
    }
}

impl axum::response::IntoResponse for ProtectedStreamSession {
    fn into_response(self) -> axum::response::Response {
        (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/event-stream; charset=utf-8",
            )],
            axum::body::Body::new(self.into_sse_body()),
        )
            .into_response()
    }
}

impl std::fmt::Debug for ProtectedStreamSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtectedStreamSession")
            .field("citations", &self.citations.len())
            .field("warnings", &self.warnings.len())
            .field("conflict_warnings", &self.conflict_warnings.len())
            .field("version_context", &self.version_context.mode)
            .field("audit", &self.audit)
            .field("grounded", &self.grounded)
            .field("mode", &self.mode)
            .field("receiver", &"ProtectedStreamReceiver")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QaAuditMetadata {
    pub action: &'static str,
    pub outcome: &'static str,
    pub answer_mode: &'static str,
    pub citation_count: usize,
    pub conflict_warning_count: usize,
    pub version_mode: &'static str,
    pub provider_configured: bool,
    pub fallback_reason: Option<&'static str>,
    pub request_id: String,
    pub grounded: bool,
}

impl QaAuditMetadata {
    pub fn to_json(&self) -> JsonValue {
        json!({
            "action": self.action,
            "outcome": self.outcome,
            "answer_mode": self.answer_mode,
            "citation_count": self.citation_count,
            "conflict_warning_count": self.conflict_warning_count,
            "version_mode": self.version_mode,
            "provider_configured": self.provider_configured,
            "fallback_reason": self.fallback_reason,
            "request_id": self.request_id,
            "grounded": self.grounded,
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QaError {
    #[error("invalid qa request")]
    InvalidRequest(&'static str),
    #[error("permission denied")]
    PermissionDenied,
    #[error("stale retrieval provenance")]
    StaleRetrieval,
    #[error("grounding error")]
    Grounding(#[from] GroundingError),
    #[error("provider error")]
    Provider(#[from] ProviderError),
    #[error("citation error")]
    Citation,
    #[error("database error")]
    Database,
}

impl QaError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "qa_invalid_request",
            Self::PermissionDenied => "qa_permission_denied",
            Self::StaleRetrieval => "qa_stale_retrieval",
            Self::Grounding(error) => error.code(),
            Self::Provider(error) => error.code(),
            Self::Citation => "qa_citation_error",
            Self::Database => "qa_database",
        }
    }
}

impl From<CitationError> for QaError {
    fn from(_: CitationError) -> Self {
        Self::Citation
    }
}

fn version_mode_label(mode: &VersionMode) -> &'static str {
    match mode {
        VersionMode::Current => "current",
        VersionMode::AsOf { .. } => "as_of",
        VersionMode::Compare { .. } => "compare",
        VersionMode::History { .. } => "history",
    }
}

fn new_request_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// Validates retrieval provenance against the live OrgContext and request.
///
/// Provenance `collection_ids` must equal the resolved requested scope exactly
/// (not a superset). Document filter for compare/history must match.
pub fn bind_retrieval_provenance(
    ctx: &OrgContext,
    request: &QaRequest,
    retrieval: &RetrievalResponse,
) -> Result<(), QaError> {
    let prov = &retrieval.provenance;
    if prov.org_id != ctx.org_id() || prov.user_id != ctx.user_id() {
        return Err(QaError::StaleRetrieval);
    }
    if prov.mode != request.mode {
        return Err(QaError::StaleRetrieval);
    }
    if Utc::now()
        .signed_duration_since(prov.retrieved_at)
        .to_std()
        .unwrap_or(Duration::MAX)
        > STALE_RETRIEVAL_MAX_AGE
    {
        return Err(QaError::StaleRetrieval);
    }
    let scope = resolve_scope(ctx, request.collection_ids.as_ref())
        .map_err(|_| QaError::PermissionDenied)?;
    if prov.collection_ids != scope.collection_ids {
        return Err(QaError::StaleRetrieval);
    }
    let expected_doc = match &request.mode {
        VersionMode::Compare { document_id, .. } | VersionMode::History { document_id, .. } => {
            Some(*document_id)
        }
        _ => None,
    };
    if prov.document_id != expected_doc {
        return Err(QaError::StaleRetrieval);
    }
    for hit in &retrieval.hits {
        if !scope.collection_ids.contains(&hit.collection_id) {
            return Err(QaError::StaleRetrieval);
        }
        if let Some(doc) = expected_doc {
            if hit.document_id != doc {
                return Err(QaError::StaleRetrieval);
            }
        }
    }
    Ok(())
}

fn enforce_bounds(question: &str, hits: &[RetrievalHit]) -> Result<(), QaError> {
    if question.trim().is_empty() {
        return Err(QaError::InvalidRequest("question is empty"));
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return Err(QaError::InvalidRequest("question too long"));
    }
    if hits.len() > MAX_PASSAGES {
        return Err(QaError::InvalidRequest("too many passages"));
    }
    let mut total_bytes = 0usize;
    for hit in hits {
        if hit.snippet.chars().count() > MAX_PASSAGE_CHARS
            || hit.body.chars().count() > MAX_PASSAGE_CHARS
        {
            return Err(QaError::InvalidRequest("passage too long"));
        }
        total_bytes = total_bytes
            .saturating_add(hit.snippet.len())
            .saturating_add(hit.body.len());
        if total_bytes > MAX_TOTAL_PASSAGE_BYTES {
            return Err(QaError::InvalidRequest("passage bytes exceed cap"));
        }
    }
    Ok(())
}

fn prompt_passages(passages: &[GroundingPassage]) -> Vec<PromptPassage> {
    passages
        .iter()
        .map(|passage| PromptPassage {
            cite_id: passage.cite_id.clone(),
            source_label: passage.hit.document_id.to_string(),
            heading: passage.hit.heading.clone(),
            // Prefer authoritative quote for framing when available.
            snippet: passage
                .authoritative_quote
                .clone()
                .unwrap_or_else(|| passage.hit.snippet.clone()),
            version_number: passage.hit.version_number,
            is_current: passage.hit.is_current,
        })
        .collect()
}

async fn attach_authoritative_quotes<S: BlobStore>(
    pool: &Pool,
    storage: &S,
    ctx: &OrgContext,
    passages: &mut [GroundingPassage],
) -> Result<(), QaError> {
    let requests: Vec<CitationResolveRequest> = passages
        .iter()
        .map(|passage| CitationResolveRequest {
            chunk_id: passage.hit.chunk_id,
            expected_version_id: Some(passage.hit.version_id),
            expected_document_id: Some(passage.hit.document_id),
            expected_content_sha256: Some(passage.hit.content_sha256.clone()),
            expected_quote: None,
            expected_span_start: None,
            expected_span_end: None,
        })
        .collect();
    let citations = resolve_citations_batched(pool, storage, ctx, &requests).await?;
    if citations.len() != passages.len() {
        return Err(QaError::Citation);
    }
    for (passage, citation) in passages.iter_mut().zip(citations) {
        passage.authoritative_quote = Some(citation.quote);
    }
    Ok(())
}

/// Canonical cite-id → StableCitation map (order of cited_ids preserved).
async fn resolve_answer_citations<S: BlobStore>(
    pool: &Pool,
    storage: &S,
    ctx: &OrgContext,
    passages: &[GroundingPassage],
    cited_ids: &[String],
) -> Result<Vec<StableCitation>, QaError> {
    let by_id = GroundingPassage::by_cite_id(passages);
    let mut requests = Vec::with_capacity(cited_ids.len());
    for cite in cited_ids {
        let passage = by_id
            .get(cite)
            .ok_or(QaError::Grounding(GroundingError::FabricatedCitation))?;
        requests.push(CitationResolveRequest {
            chunk_id: passage.hit.chunk_id,
            expected_version_id: Some(passage.hit.version_id),
            expected_document_id: Some(passage.hit.document_id),
            expected_content_sha256: Some(passage.hit.content_sha256.clone()),
            expected_quote: passage.authoritative_quote.clone(),
            expected_span_start: None,
            expected_span_end: None,
        });
    }
    resolve_citations_batched(pool, storage, ctx, &requests)
        .await
        .map_err(|_| QaError::Citation)
}

fn map_conflicts(rows: &[search::QaConflictRow], passages: &[GroundingPassage]) -> Vec<QaConflict> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let claim_a_cite = passages
            .iter()
            .find(|p| {
                p.hit.version_id == row.claim_a_version_id
                    && row.claim_a_chunk_id.is_none_or(|id| id == p.hit.chunk_id)
            })
            .map(|p| p.cite_id.clone());
        let claim_b_cite = passages
            .iter()
            .find(|p| {
                p.hit.version_id == row.claim_b_version_id
                    && row.claim_b_chunk_id.is_none_or(|id| id == p.hit.chunk_id)
            })
            .map(|p| p.cite_id.clone());
        let (Some(claim_a_cite_id), Some(claim_b_cite_id)) = (claim_a_cite, claim_b_cite) else {
            // Unrelated / unmapped evidence → omit (not global failure).
            continue;
        };
        if matches!(row.status, crate::db::models::ConflictStatus::Resolved)
            && (!row.resolution_a_authorized || !row.resolution_b_authorized)
        {
            continue;
        }
        if matches!(
            row.status,
            crate::db::models::ConflictStatus::AcceptedException
                | crate::db::models::ConflictStatus::FalsePositive
        ) && (row.resolution_version_a_id.is_some() || row.resolution_version_b_id.is_some())
        {
            continue;
        }
        // H8: resolution cites must pin exact resolution claim chunks.
        let resolution_a_cite_id = row.resolution_a_chunk_id.and_then(|cid| {
            passages
                .iter()
                .find(|p| p.hit.chunk_id == cid)
                .map(|p| p.cite_id.clone())
        });
        let resolution_b_cite_id = row.resolution_b_chunk_id.and_then(|cid| {
            passages
                .iter()
                .find(|p| p.hit.chunk_id == cid)
                .map(|p| p.cite_id.clone())
        });
        if matches!(row.status, crate::db::models::ConflictStatus::Resolved) {
            let (Some(_), Some(_)) = (&resolution_a_cite_id, &resolution_b_cite_id) else {
                continue;
            };
            // H8: normalized resolution values must align when both sides are numeric.
            if matches!(row.conflict_type, crate::db::models::ConflictType::Numeric) {
                match (row.resolution_a_number, row.resolution_b_number) {
                    (Some(a), Some(b)) if a.normalize() != b.normalize() => continue,
                    (None, _) | (_, None) => continue,
                    _ => {}
                }
            }
        }
        out.push(QaConflict {
            conflict_id: row.conflict_id,
            status: row.status,
            conflict_type: row.conflict_type,
            severity: row.severity,
            claim_a_id: row.claim_a_id,
            claim_b_id: row.claim_b_id,
            claim_a_key: row.claim_a_key.clone(),
            claim_b_key: row.claim_b_key.clone(),
            claim_a_scope: row.claim_a_scope.clone(),
            claim_b_scope: row.claim_b_scope.clone(),
            claim_a_unit: row.claim_a_unit.clone(),
            claim_b_unit: row.claim_b_unit.clone(),
            claim_a_version_id: row.claim_a_version_id,
            claim_b_version_id: row.claim_b_version_id,
            claim_a_document_id: row.claim_a_document_id,
            claim_b_document_id: row.claim_b_document_id,
            claim_a_is_current: row.claim_a_is_current,
            claim_b_is_current: row.claim_b_is_current,
            claim_a_quote: row.claim_a_quote.clone(),
            claim_b_quote: row.claim_b_quote.clone(),
            claim_a_cite_id,
            claim_b_cite_id,
            claim_a_numeric: row.claim_a_number,
            claim_b_numeric: row.claim_b_number,
            resolution_note: row.resolution_note.clone(),
            resolution_version_a_id: row.resolution_version_a_id,
            resolution_version_b_id: row.resolution_version_b_id,
            resolution_a_cite_id,
            resolution_b_cite_id,
        });
    }
    out
}

struct BuildAnswer<'a> {
    answer: String,
    citations: Vec<StableCitation>,
    mode: AnswerMode,
    grounded: bool,
    warnings: Vec<String>,
    version_context: VersionContext,
    conflict_warnings: Vec<ConflictWarning>,
    provider_configured: bool,
    fallback_reason: Option<&'static str>,
    request_id: &'a str,
}

fn finish_answer(input: BuildAnswer<'_>) -> QaAnswer {
    let audit = QaAuditMetadata {
        action: "qa.ask",
        outcome: if matches!(input.mode, AnswerMode::FallbackExtractive) {
            "fallback"
        } else if input.grounded {
            "success"
        } else {
            "ungrounded"
        },
        answer_mode: input.mode.as_str(),
        citation_count: input.citations.len(),
        conflict_warning_count: input.conflict_warnings.len(),
        version_mode: input.version_context.mode,
        provider_configured: input.provider_configured,
        fallback_reason: input.fallback_reason,
        request_id: input.request_id.to_string(),
        grounded: input.grounded,
    };
    QaAnswer {
        answer: input.answer,
        citations: input.citations,
        mode: input.mode,
        grounded: input.grounded,
        warnings: input.warnings,
        version_context: input.version_context,
        conflict_warnings: input.conflict_warnings,
        audit,
    }
}

/// Core ask path with pool/storage for R02 citation + fresh conflict/history loads.
#[allow(clippy::too_many_arguments)]
pub async fn answer_question_live<S: BlobStore, P: QaChatProvider>(
    pool: &Pool,
    lock_pool: &LockPool,
    storage: &S,
    ctx: &OrgContext,
    request: QaRequest,
    mut retrieval: RetrievalResponse,
    provider: Option<&P>,
    provider_config: Option<&QaProviderConfig>,
    cancel: Option<&StreamCancel>,
) -> Result<GuardedQaResponse, QaError> {
    let request_id = new_request_id();
    require_permission(ctx, PERMISSION_QA_QUERY).map_err(|_| QaError::PermissionDenied)?;
    if !matches!(request.mode, VersionMode::Current) {
        require_permission(ctx, PERMISSION_QA_HISTORY).map_err(|_| QaError::PermissionDenied)?;
    }
    let fresh = resolve_org_context_in_txn(pool, ctx.org_id(), ctx.user_id())
        .await
        .map_err(|_| QaError::PermissionDenied)?;
    bind_retrieval_provenance(&fresh, &request, &retrieval)?;
    enforce_bounds(&request.question, &retrieval.hits)?;

    let scope = resolve_scope(&fresh, request.collection_ids.as_ref())
        .map_err(|_| QaError::PermissionDenied)?;
    let collection_ids: Vec<Uuid> = scope.collection_ids.iter().copied().collect();
    retrieval
        .hits
        .retain(|hit| scope.collection_ids.contains(&hit.collection_id));
    if matches!(request.mode, VersionMode::Current) {
        retrieval.hits.retain(|hit| hit.is_current);
    }
    if let Some(doc) = retrieval.provenance.document_id {
        retrieval.hits.retain(|hit| hit.document_id == doc);
    }

    let mut passages = GroundingPassage::from_hits(&retrieval.hits);

    // Compare/history: ensure representative authoritative evidence per required version.
    match &request.mode {
        VersionMode::Compare {
            document_id,
            version_a,
            version_b,
        } => {
            let required = [*version_a, *version_b];
            if ensure_versions_represented(&passages, &required).is_err() {
                let rows = with_org_txn(pool, &fresh, {
                    let fresh = fresh.clone();
                    let collection_ids = collection_ids.clone();
                    let document_id = *document_id;
                    move |txn| {
                        Box::pin(async move {
                            search::load_representative_chunks_for_versions(
                                txn,
                                &fresh,
                                document_id,
                                &required,
                                &collection_ids,
                            )
                            .await
                        })
                    }
                })
                .await
                .map_err(|_| QaError::Database)?;
                let extra_hits = representative_to_hits(&rows);
                let mut merged = retrieval.hits.clone();
                for hit in extra_hits {
                    if !merged.iter().any(|h| h.version_id == hit.version_id) {
                        merged.push(hit);
                    }
                }
                passages = GroundingPassage::from_hits(&merged);
            }
            passages = filter_compare_passages(&passages, *version_a, *version_b)?;
            ensure_versions_represented(&passages, &required)?;
        }
        VersionMode::History {
            document_id,
            before_version_no,
        } => {
            let timeline_probe = with_org_txn(pool, &fresh, {
                let fresh = fresh.clone();
                let collection_ids = collection_ids.clone();
                let document_id = *document_id;
                let before_version_no = *before_version_no;
                move |txn| {
                    Box::pin(async move {
                        // MAX+1 newest-first for truncation detection.
                        search::list_published_version_timeline_recent_page(
                            txn,
                            &fresh,
                            document_id,
                            &collection_ids,
                            (MAX_HISTORY_VERSIONS + 1) as i64,
                            before_version_no,
                        )
                        .await
                    })
                }
            })
            .await
            .map_err(|_| QaError::Database)?;
            let (paged, _page_meta) =
                page_history_timeline(timeline_probe, MAX_HISTORY_VERSIONS, *before_version_no)?;
            // H7: evidence restricted to this page's version IDs (current loaded separately).
            let required: Vec<Uuid> = paged.iter().map(|r| r.version_id).collect();
            // M11: hydrate terminal 1-version history when evidence missing.
            if !required.is_empty() && ensure_versions_represented(&passages, &required).is_err() {
                let rows = with_org_txn(pool, &fresh, {
                    let fresh = fresh.clone();
                    let collection_ids = collection_ids.clone();
                    let document_id = *document_id;
                    let required = required.clone();
                    move |txn| {
                        Box::pin(async move {
                            search::load_representative_chunks_for_versions(
                                txn,
                                &fresh,
                                document_id,
                                &required,
                                &collection_ids,
                            )
                            .await
                        })
                    }
                })
                .await
                .map_err(|_| QaError::Database)?;
                let extra_hits = representative_to_hits(&rows);
                let mut merged = retrieval.hits.clone();
                for hit in extra_hits {
                    if !merged.iter().any(|h| h.chunk_id == hit.chunk_id) {
                        merged.push(hit);
                    }
                }
                enforce_bounds(&request.question, &merged)?;
                passages = GroundingPassage::from_hits(&merged);
            }
        }
        _ => {}
    }

    if passages.is_empty() {
        let answer = finish_answer(BuildAnswer {
            answer: extractive_answer(&request.question, &[]),
            citations: vec![],
            mode: AnswerMode::OfflineExtractive,
            grounded: false,
            warnings: vec!["Không có bằng chứng được ủy quyền.".into()],
            version_context: VersionContext {
                mode: version_mode_label(&request.mode),
                current_version_ids: vec![],
                cited_version_ids: vec![],
                change_note: None,
                history: vec![],
                history_page: None,
            },
            conflict_warnings: vec![],
            provider_configured: provider_config.is_some(),
            fallback_reason: Some("empty_evidence"),
            request_id: &request_id,
        });
        let colls: Vec<Uuid> = collection_ids.clone();
        let (guard, snapshot) =
            acquire_final_delivery_guard(lock_pool, pool, ctx, &answer, &colls).await?;
        return GuardedQaResponse::from_answer(answer, guard, snapshot);
    }

    attach_authoritative_quotes(pool, storage, &fresh, &mut passages).await?;

    let evidence_docs: BTreeSet<Uuid> = passages.iter().map(|p| p.hit.document_id).collect();
    let evidence_versions: BTreeSet<Uuid> = passages.iter().map(|p| p.hit.version_id).collect();
    let evidence_doc_vec: Vec<Uuid> = evidence_docs.iter().copied().collect();
    let evidence_ver_vec: Vec<Uuid> = evidence_versions.iter().copied().collect();
    let current_only = matches!(request.mode, VersionMode::Current);
    let as_of = match &request.mode {
        VersionMode::AsOf { at } => Some(*at),
        _ => None,
    };
    let conflict_rows = with_org_txn(pool, &fresh, {
        let fresh = fresh.clone();
        let collection_ids = collection_ids.clone();
        let evidence_doc_vec = evidence_doc_vec.clone();
        let evidence_ver_vec = evidence_ver_vec.clone();
        move |txn| {
            Box::pin(async move {
                search::load_qa_conflicts_for_evidence(
                    txn,
                    &fresh,
                    &collection_ids,
                    &evidence_doc_vec,
                    &evidence_ver_vec,
                    current_only,
                    as_of,
                    MAX_CONFLICTS,
                )
                .await
            })
        }
    })
    .await
    .map_err(|_| QaError::Database)?;
    // Cross-doc conflict evidence: exact claim + resolution claim chunks (H8).
    {
        let mut exact_chunk_ids: BTreeSet<Uuid> = BTreeSet::new();
        let mut needed_by_doc: std::collections::BTreeMap<Uuid, BTreeSet<Uuid>> =
            std::collections::BTreeMap::new();
        for row in &conflict_rows {
            if let Some(cid) = row.claim_a_chunk_id {
                exact_chunk_ids.insert(cid);
            }
            if let Some(cid) = row.claim_b_chunk_id {
                exact_chunk_ids.insert(cid);
            }
            if let Some(cid) = row.resolution_a_chunk_id {
                exact_chunk_ids.insert(cid);
            }
            if let Some(cid) = row.resolution_b_chunk_id {
                exact_chunk_ids.insert(cid);
            }
            needed_by_doc
                .entry(row.claim_a_document_id)
                .or_default()
                .insert(row.claim_a_version_id);
            needed_by_doc
                .entry(row.claim_b_document_id)
                .or_default()
                .insert(row.claim_b_version_id);
            if matches!(row.status, crate::db::models::ConflictStatus::Resolved) {
                if let Some(vid) = row.resolution_version_a_id {
                    needed_by_doc
                        .entry(row.claim_a_document_id)
                        .or_default()
                        .insert(vid);
                }
                if let Some(vid) = row.resolution_version_b_id {
                    needed_by_doc
                        .entry(row.claim_b_document_id)
                        .or_default()
                        .insert(vid);
                }
            }
        }
        let missing_exact: Vec<Uuid> = exact_chunk_ids
            .into_iter()
            .filter(|cid| !passages.iter().any(|p| p.hit.chunk_id == *cid))
            .collect();
        let missing_versions: Vec<(Uuid, Vec<Uuid>)> = needed_by_doc
            .into_iter()
            .map(|(doc, versions)| {
                let missing: Vec<Uuid> = versions
                    .into_iter()
                    .filter(|vid| !passages.iter().any(|p| p.hit.version_id == *vid))
                    .collect();
                (doc, missing)
            })
            .filter(|(_, v)| !v.is_empty())
            .collect();
        if !missing_exact.is_empty() || !missing_versions.is_empty() {
            let mut merged: Vec<_> = passages.iter().map(|p| p.hit.clone()).collect();
            if !missing_exact.is_empty() {
                let rows = with_org_txn(pool, &fresh, {
                    let fresh = fresh.clone();
                    let missing_exact = missing_exact.clone();
                    move |txn| {
                        Box::pin(async move {
                            search::hydrate_chunks_for_citation(txn, &fresh, &missing_exact).await
                        })
                    }
                })
                .await
                .map_err(|_| QaError::Database)?;
                for row in rows {
                    let hit = hydrated_to_hit(&row);
                    if !merged.iter().any(|h| h.chunk_id == hit.chunk_id) {
                        merged.push(hit);
                    }
                }
            }
            for (document_id, version_ids) in missing_versions {
                let rows = with_org_txn(pool, &fresh, {
                    let fresh = fresh.clone();
                    let collection_ids = collection_ids.clone();
                    move |txn| {
                        Box::pin(async move {
                            search::load_representative_chunks_for_versions(
                                txn,
                                &fresh,
                                document_id,
                                &version_ids,
                                &collection_ids,
                            )
                            .await
                        })
                    }
                })
                .await
                .map_err(|_| QaError::Database)?;
                for hit in representative_to_hits(&rows) {
                    if !merged.iter().any(|h| h.chunk_id == hit.chunk_id) {
                        merged.push(hit);
                    }
                }
            }
            enforce_bounds(&request.question, &merged)?;
            passages = GroundingPassage::from_hits(&merged);
            attach_authoritative_quotes(pool, storage, &fresh, &mut passages).await?;
        }
    }
    let _early_conflicts = map_conflicts(&conflict_rows, &passages);

    let mut history_page: Option<HistoryPageMeta> = None;
    let timeline = if let VersionMode::History {
        document_id,
        before_version_no,
    } = &request.mode
    {
        let document_id = *document_id;
        let before_version_no = *before_version_no;
        let rows = with_org_txn(pool, &fresh, {
            let fresh = fresh.clone();
            let collection_ids = collection_ids.clone();
            move |txn| {
                Box::pin(async move {
                    search::list_published_version_timeline_recent_page(
                        txn,
                        &fresh,
                        document_id,
                        &collection_ids,
                        (MAX_HISTORY_VERSIONS + 1) as i64,
                        before_version_no,
                    )
                    .await
                })
            }
        })
        .await
        .map_err(|_| QaError::Database)?;
        let (paged, meta) = page_history_timeline(rows, MAX_HISTORY_VERSIONS, before_version_no)?;
        if paged.is_empty() {
            return Err(QaError::Grounding(GroundingError::MixedVersionCitation));
        }
        // H7: evidence restricted to page version IDs only (not spliced current).
        let required: Vec<Uuid> = paged.iter().map(|r| r.version_id).collect();
        if ensure_versions_represented(&passages, &required).is_err() {
            let rows = with_org_txn(pool, &fresh, {
                let fresh = fresh.clone();
                let collection_ids = collection_ids.clone();
                move |txn| {
                    Box::pin(async move {
                        search::load_representative_chunks_for_versions(
                            txn,
                            &fresh,
                            document_id,
                            &required,
                            &collection_ids,
                        )
                        .await
                    })
                }
            })
            .await
            .map_err(|_| QaError::Database)?;
            let mut merged: Vec<_> = passages.iter().map(|p| p.hit.clone()).collect();
            for hit in representative_to_hits(&rows) {
                if !merged.iter().any(|h| h.chunk_id == hit.chunk_id) {
                    merged.push(hit);
                }
            }
            enforce_bounds(&request.question, &merged)?;
            passages = GroundingPassage::from_hits(&merged);
            attach_authoritative_quotes(pool, storage, &fresh, &mut passages).await?;
        }
        history_page = meta;
        // H7: drop retrieval hits outside this page's version set.
        let page_ids: BTreeSet<Uuid> = paged.iter().map(|r| r.version_id).collect();
        passages.retain(|p| page_ids.contains(&p.hit.version_id));
        paged
    } else {
        Vec::<VersionTimelineRow>::new()
    };

    let typed_delta = if let VersionMode::Compare {
        document_id,
        version_a,
        version_b,
    } = &request.mode
    {
        // Deterministic DB join by key+subject+predicate+scope+unit — no independent find.
        let pair = with_org_txn(pool, &fresh, {
            let fresh = fresh.clone();
            let collection_ids = collection_ids.clone();
            let document_id = *document_id;
            let version_a = *version_a;
            let version_b = *version_b;
            let question = request.question.clone();
            move |txn| {
                Box::pin(async move {
                    search::load_typed_delta_pair(
                        txn,
                        &fresh,
                        document_id,
                        version_a,
                        version_b,
                        &collection_ids,
                        &question,
                    )
                    .await
                })
            }
        })
        .await
        .map_err(|_| QaError::Database)?;
        if let Some(row) = pair.as_ref() {
            // M1: hydrate exact selected delta chunk IDs only.
            let need = [row.older_chunk_id, row.newer_chunk_id];
            let missing: Vec<Uuid> = need
                .iter()
                .copied()
                .filter(|cid| !passages.iter().any(|p| p.hit.chunk_id == *cid))
                .collect();
            if !missing.is_empty() {
                let rows = with_org_txn(pool, &fresh, {
                    let fresh = fresh.clone();
                    let missing = missing.clone();
                    move |txn| {
                        Box::pin(async move {
                            search::hydrate_chunks_for_citation(txn, &fresh, &missing).await
                        })
                    }
                })
                .await
                .map_err(|_| QaError::Database)?;
                let mut merged: Vec<_> = passages.iter().map(|p| p.hit.clone()).collect();
                for row in rows {
                    let hit = hydrated_to_hit(&row);
                    if need.contains(&hit.chunk_id)
                        && !merged.iter().any(|h| h.chunk_id == hit.chunk_id)
                    {
                        merged.push(hit);
                    }
                }
                enforce_bounds(&request.question, &merged)?;
                passages = GroundingPassage::from_hits(&merged);
                attach_authoritative_quotes(pool, storage, &fresh, &mut passages).await?;
            }
        }
        pair.as_ref()
            .and_then(|row| typed_delta_from_pair_row(row, &passages))
    } else {
        None
    };

    let prompt_passages = prompt_passages(&passages);
    debug_assert!(system_policy_is_separated(
        &request.question,
        &prompt_passages
    ));
    let user = grounded_user_prompt(&request.question, &prompt_passages);
    if user.len() > MAX_PROMPT_BYTES {
        return Err(QaError::InvalidRequest("prompt too large"));
    }

    let provider_configured = provider_config.is_some();
    let mut warnings = std::mem::take(&mut retrieval.warnings);

    let (structured, mode, fallback_reason, require_structured) = if !request.use_llm {
        let filtered: Vec<GroundingPassage> = passages_for_mode(&passages, &request.mode)
            .into_iter()
            .cloned()
            .collect();
        (
            extractive_structured_claims(&filtered),
            AnswerMode::OfflineExtractive,
            None,
            false,
        )
    } else if let Some(provider) = provider {
        let chat_request = ChatCompletionRequest {
            system: GROUNDED_SYSTEM_POLICY.to_string(),
            user,
        };
        let timeout_budget = provider_config
            .map(QaProviderConfig::timeout)
            .unwrap_or(Duration::from_secs(30));
        // H1/H4: capture epoch → shared guard → exact auth+epoch compare on guard
        // connection → provider egress (guard held through provider).
        let evidence_docs: Vec<Uuid> = passages
            .iter()
            .map(|p| p.hit.document_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let evidence_versions: Vec<Uuid> = passages
            .iter()
            .map(|p| p.hit.version_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let pre_provider_epoch = with_org_txn(pool, &fresh, {
            let org_id = ctx.org_id();
            let user_id = ctx.user_id();
            let docs = evidence_docs.clone();
            move |txn| {
                Box::pin(async move {
                    let _ = authz_epoch::ensure_user_epoch(txn, org_id, user_id).await?;
                    authz_epoch::read_epoch_snapshot(txn, org_id, user_id, &docs).await
                })
            }
        })
        .await
        .map_err(|_| QaError::Database)?;
        let captured_pre = pre_provider_epoch.composite();
        let delivery_guard = crate::db::authz_lock::DeliveryGuard::acquire_shared(
            lock_pool,
            ctx.org_id(),
            ctx.user_id(),
            &evidence_docs,
            &collection_ids,
        )
        .await
        .map_err(|_| QaError::PermissionDenied)?;
        let require_history = !matches!(request.mode, VersionMode::Current);
        {
            let client = match delivery_guard.client() {
                Ok(c) => c,
                Err(_) => {
                    delivery_guard.abandon().await;
                    return Err(QaError::PermissionDenied);
                }
            };
            let probe = search::probe_stream_authz_exact(
                client,
                ctx.org_id(),
                ctx.user_id(),
                &evidence_docs,
                &evidence_versions,
                &collection_ids,
                require_history,
            )
            .await
            .map_err(|_| QaError::PermissionDenied)?;
            if !matches!(probe, search::StreamAuthzProbe::Allow) {
                delivery_guard.abandon().await;
                return Err(QaError::PermissionDenied);
            }
            let snap = authz_epoch::read_epoch_snapshot_on_client(
                client,
                ctx.org_id(),
                ctx.user_id(),
                &evidence_docs,
            )
            .await
            .map_err(|_| QaError::PermissionDenied)?;
            if snap.composite() != captured_pre {
                delivery_guard.abandon().await;
                return Err(QaError::PermissionDenied);
            }
        }
        let provider_result = tokio::time::timeout(
            timeout_budget,
            accumulate_provider_sse(provider, &chat_request, cancel, provider_config),
        )
        .await;
        delivery_guard.release().await;
        match provider_result {
            Ok(Ok(payload)) if !payload.refusal => {
                (payload.claims, AnswerMode::ProviderLlm, None, true)
            }
            Ok(Ok(_)) => {
                warnings.push("QA provider refused; fallback extractive.".into());
                (
                    extractive_structured_claims(&passages),
                    AnswerMode::FallbackExtractive,
                    Some("provider_refusal"),
                    false,
                )
            }
            Ok(Err(ProviderError::Cancelled)) => {
                return Err(QaError::Provider(ProviderError::Cancelled));
            }
            Ok(Err(ProviderError::Timeout)) | Err(_) => {
                warnings.push("QA provider timeout; fallback extractive.".into());
                (
                    extractive_structured_claims(&passages),
                    AnswerMode::FallbackExtractive,
                    Some("provider_timeout"),
                    false,
                )
            }
            Ok(Err(_)) => {
                warnings.push("QA provider outage; fallback extractive.".into());
                (
                    extractive_structured_claims(&passages),
                    AnswerMode::FallbackExtractive,
                    Some("provider_outage"),
                    false,
                )
            }
        }
    } else {
        warnings.push("QA provider unavailable; fallback extractive.".into());
        (
            extractive_structured_claims(&passages),
            AnswerMode::FallbackExtractive,
            Some("provider_unavailable"),
            false,
        )
    };

    let (structured, mode, fallback_reason) = match validate_structured_claims(
        &structured,
        &passages,
        &request.mode,
        require_structured || !structured.is_empty(),
    ) {
        Ok(_) => (structured, mode, fallback_reason),
        Err(error) if matches!(mode, AnswerMode::ProviderLlm) => {
            warnings.push(format!(
                "Grounding không hợp lệ ({}); fallback extractive.",
                error.code()
            ));
            let filtered: Vec<GroundingPassage> = passages_for_mode(&passages, &request.mode)
                .into_iter()
                .cloned()
                .collect();
            (
                extractive_structured_claims(&filtered),
                AnswerMode::FallbackExtractive,
                Some(error.code()),
            )
        }
        Err(error) => return Err(QaError::Grounding(error)),
    };

    let mut answer = if matches!(
        mode,
        AnswerMode::OfflineExtractive | AnswerMode::FallbackExtractive
    ) {
        let filtered: Vec<GroundingPassage> = passages_for_mode(&passages, &request.mode)
            .into_iter()
            .cloned()
            .collect();
        extractive_answer(&request.question, &filtered)
    } else {
        render_answer_from_claims(&structured)
    };
    let cited = validate_structured_claims(
        &structured,
        &passages,
        &request.mode,
        !structured.is_empty(),
    )?;

    let mut version_context = build_version_context(
        &request.mode,
        &passages,
        &cited,
        &timeline,
        typed_delta.as_ref(),
        history_page.clone(),
        None,
    )?;
    let mut server_notes: Vec<ServerNote> = Vec::new();
    if let Some(note) = version_context.change_note.clone() {
        server_notes.push(ServerNote {
            kind: if matches!(request.mode, VersionMode::History { .. }) {
                ServerNoteKind::History
            } else {
                ServerNoteKind::Change
            },
            message: note,
            // H10: every typed ServerNote owns cite IDs.
            pin_cite_ids: cited.clone(),
        });
    }

    let doc_ids: Vec<Uuid> = dedup_sorted_uuids(&evidence_docs.iter().copied().collect::<Vec<_>>());
    let version_ids_for_cites: Vec<Uuid> = dedup_sorted_uuids(
        &cited
            .iter()
            .filter_map(|c| {
                passages
                    .iter()
                    .find(|p| p.cite_id == *c)
                    .map(|p| p.hit.version_id)
            })
            .collect::<Vec<_>>(),
    );
    let evidence_ver_vec = dedup_sorted_uuids(&evidence_ver_vec);
    let evidence_doc_vec = dedup_sorted_uuids(&evidence_doc_vec);
    let as_of_final = as_of;
    let require_history_final = !matches!(request.mode, VersionMode::Current);
    // Capture epoch before final serialized barrier for compare (H8).
    let captured_epoch = with_org_txn(pool, &fresh, {
        let org_id = ctx.org_id();
        let user_id = ctx.user_id();
        let doc_ids = doc_ids.clone();
        move |txn| {
            Box::pin(async move {
                let _ = authz_epoch::ensure_user_epoch(txn, org_id, user_id).await?;
                authz_epoch::read_epoch_snapshot(txn, org_id, user_id, &doc_ids).await
            })
        }
    })
    .await
    .map_err(|_| QaError::Database)?;
    let captured_composite = captured_epoch.composite();
    let history_cursor = match &request.mode {
        VersionMode::History {
            before_version_no, ..
        } => *before_version_no,
        _ => None,
    };
    let history_document_id = match &request.mode {
        VersionMode::History { document_id, .. } => Some(*document_id),
        _ => None,
    };
    let timeline_version_ids: Vec<Uuid> = timeline.iter().map(|r| r.version_id).collect();
    let history_page_for_final = history_page.clone();
    // H8: SERIALIZABLE final txn — re-resolve membership/perms/ACL/mode pointers/
    // timeline/conflicts and compare epochs under one snapshot.
    let final_bundle = with_org_txn_serializable(pool, &fresh, {
        let org_id = ctx.org_id();
        let user_id = ctx.user_id();
        let request_mode = request.mode.clone();
        let passages = passages.clone();
        let collection_ids = collection_ids.clone();
        let cited = cited.clone();
        let evidence_doc_vec = evidence_doc_vec.clone();
        let evidence_ver_vec = evidence_ver_vec.clone();
        let doc_ids = doc_ids.clone();
        let version_ids_for_cites = version_ids_for_cites.clone();
        let timeline_version_ids = timeline_version_ids.clone();
        let history_page_for_final = history_page_for_final.clone();
        move |txn| {
            Box::pin(async move {
                let probe = search::probe_stream_authz_exact(
                    txn,
                    org_id,
                    user_id,
                    &doc_ids,
                    &version_ids_for_cites,
                    &collection_ids,
                    require_history_final,
                )
                .await?;
                if !matches!(probe, search::StreamAuthzProbe::Allow) {
                    return Err(crate::db::error::DbError::Config(
                        "qa final auth denied".into(),
                    ));
                }
                let snap = authz_epoch::read_epoch_snapshot(txn, org_id, user_id, &doc_ids).await?;
                if snap.composite() != captured_composite {
                    return Err(crate::db::error::DbError::Config("qa epoch race".into()));
                }
                // Re-resolve membership + permission set (exact qa.query / qa.history).
                let membership = txn
                    .query_opt(
                        "SELECT m.role
                         FROM org_memberships m
                         JOIN users u ON u.id = m.user_id
                         WHERE m.org_id = $1 AND m.user_id = $2 AND u.disabled_at IS NULL",
                        &[&org_id, &user_id],
                    )
                    .await?;
                if membership.is_none() {
                    return Err(crate::db::error::DbError::Config(
                        "qa final auth denied".into(),
                    ));
                }
                let mut perms = vec![PERMISSION_QA_QUERY];
                if require_history_final {
                    perms.push(PERMISSION_QA_HISTORY);
                }
                let ctx2 = OrgContext::try_new(org_id, user_id, perms, collection_ids.clone())
                    .map_err(|e| crate::db::error::DbError::Config(e.to_string()))?;
                match &request_mode {
                    VersionMode::Current => {
                        let ok = search::verify_versions_still_current(
                            txn,
                            &ctx2,
                            &version_ids_for_cites,
                            &collection_ids,
                        )
                        .await?;
                        if !ok {
                            return Err(crate::db::error::DbError::Config(
                                "qa current pointer race".into(),
                            ));
                        }
                    }
                    VersionMode::AsOf { at } => {
                        let ok = search::verify_citation_pins_still_valid(
                            txn,
                            &ctx2,
                            &version_ids_for_cites,
                            &collection_ids,
                            false,
                            Some(*at),
                        )
                        .await?;
                        if !ok {
                            return Err(crate::db::error::DbError::Config(
                                "qa as_of pin race".into(),
                            ));
                        }
                    }
                    VersionMode::History { document_id, .. } => {
                        // R9.7: same MAX+1 page/cursor/truncation handling; compare IDs+meta.
                        let fresh_timeline = search::list_published_version_timeline_recent_page(
                            txn,
                            &ctx2,
                            *document_id,
                            &collection_ids,
                            (MAX_HISTORY_VERSIONS + 1) as i64,
                            history_cursor,
                        )
                        .await?;
                        let (fresh_paged, fresh_meta) = page_history_timeline(
                            fresh_timeline,
                            MAX_HISTORY_VERSIONS,
                            history_cursor,
                        )
                        .map_err(|e| crate::db::error::DbError::Config(e.to_string()))?;
                        let fresh_ids: BTreeSet<Uuid> =
                            fresh_paged.iter().map(|r| r.version_id).collect();
                        let prior: BTreeSet<Uuid> = timeline_version_ids.iter().copied().collect();
                        if fresh_ids != prior || fresh_meta != history_page_for_final {
                            return Err(crate::db::error::DbError::Config(
                                "qa timeline race".into(),
                            ));
                        }
                    }
                    VersionMode::Compare {
                        document_id,
                        version_a,
                        version_b,
                    } => {
                        let lineage = search::load_lineage_versions(
                            txn,
                            &ctx2,
                            *document_id,
                            &[*version_a, *version_b],
                            &collection_ids,
                        )
                        .await?;
                        if lineage.len() != 2 {
                            return Err(crate::db::error::DbError::Config(
                                "qa lineage race".into(),
                            ));
                        }
                    }
                }
                let current_only = matches!(request_mode, VersionMode::Current);
                let rows = search::load_qa_conflicts_for_evidence(
                    txn,
                    &ctx2,
                    &collection_ids,
                    &evidence_doc_vec,
                    &evidence_ver_vec,
                    current_only,
                    as_of_final,
                    MAX_CONFLICTS,
                )
                .await?;
                let conflicts = map_conflicts(&rows, &passages);
                let conflict_warnings = conflict_messages(&request_mode, &conflicts);
                let mut pin_cites = cited;
                for warning in &conflict_warnings {
                    for pin in &warning.pin_cite_ids {
                        if !pin_cites.iter().any(|c| c == pin) {
                            pin_cites.push(pin.clone());
                        }
                    }
                }
                // M5: AsOf separately loads current pointers (not as-of cited versions).
                let current_pointers = if matches!(request_mode, VersionMode::AsOf { .. }) {
                    Some(
                        search::load_current_published_version_ids(
                            txn,
                            &ctx2,
                            &evidence_doc_vec,
                            &collection_ids,
                        )
                        .await?,
                    )
                } else if let Some(doc) = history_document_id {
                    Some(
                        search::load_current_published_version_ids(
                            txn,
                            &ctx2,
                            &[doc],
                            &collection_ids,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                Ok((pin_cites, conflict_warnings, conflicts, current_pointers))
            })
        }
    })
    .await
    .map_err(|err| {
        let msg = format!("{err}");
        if msg.contains("final auth denied") || msg.contains("qa epoch race") {
            QaError::PermissionDenied
        } else if msg.contains("pointer race")
            || msg.contains("pin race")
            || msg.contains("timeline race")
            || msg.contains("lineage race")
        {
            QaError::StaleRetrieval
        } else {
            QaError::Database
        }
    })?;
    let (cited, conflict_warnings, conflicts, current_pointers) = final_bundle;
    for warning in &conflict_warnings {
        warnings.push(warning.message.clone());
        server_notes.push(ServerNote {
            kind: match warning.status {
                crate::db::models::ConflictStatus::Open => ServerNoteKind::ConflictOpen,
                crate::db::models::ConflictStatus::Resolved => ServerNoteKind::ConflictResolved,
                crate::db::models::ConflictStatus::AcceptedException => {
                    ServerNoteKind::ConflictAcceptedException
                }
                crate::db::models::ConflictStatus::FalsePositive => {
                    ServerNoteKind::ConflictFalsePositive
                }
            },
            message: warning.message.clone(),
            pin_cite_ids: warning.pin_cite_ids.clone(),
        });
    }
    answer = append_server_notes(&answer, &server_notes);

    version_context = build_version_context(
        &request.mode,
        &passages,
        &cited,
        &timeline,
        typed_delta.as_ref(),
        history_page.clone(),
        current_pointers,
    )?;

    let claim_cite_ids = cited.clone();
    // H10: union every typed ServerNote pin (including Change/History).
    let server_note_pin_cites: Vec<String> = server_notes
        .iter()
        .flat_map(|n| n.pin_cite_ids.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // Canonical union for hydration (one snapshot).
    let mut hydrate_cites = claim_cite_ids.clone();
    for pin in &server_note_pin_cites {
        if !hydrate_cites.iter().any(|c| c == pin) {
            hydrate_cites.push(pin.clone());
        }
    }

    let fresh2 = resolve_org_context_in_txn(pool, ctx.org_id(), ctx.user_id())
        .await
        .map_err(|_| QaError::PermissionDenied)?;
    bind_retrieval_provenance(&fresh2, &request, &retrieval)?;
    let resolved =
        resolve_answer_citations(pool, storage, &fresh2, &passages, &hydrate_cites).await?;

    // After blob verify: reacquire snapshot/epoch and verify pins current/effective/auth.
    let version_ids: Vec<Uuid> = resolved.iter().map(|c| c.version_id).collect();
    let require_current = matches!(request.mode, VersionMode::Current);
    let pins_ok = with_org_txn(pool, &fresh2, {
        let fresh2 = fresh2.clone();
        let collection_ids = collection_ids.clone();
        let version_ids = version_ids.clone();
        let doc_ids = evidence_doc_vec.clone();
        move |txn| {
            Box::pin(async move {
                let _ = authz_epoch::read_epoch_snapshot(
                    txn,
                    fresh2.org_id(),
                    fresh2.user_id(),
                    &doc_ids,
                )
                .await?;
                search::verify_citation_pins_still_valid(
                    txn,
                    &fresh2,
                    &version_ids,
                    &collection_ids,
                    require_current,
                    as_of,
                )
                .await
            })
        }
    })
    .await
    .map_err(|_| QaError::Database)?;
    if !pins_ok {
        return Err(QaError::Grounding(GroundingError::PinSnapshotRace));
    }
    if require_current && resolved.iter().any(|c| !c.is_current) {
        return Err(QaError::Grounding(GroundingError::PinSnapshotRace));
    }

    let require_structured_final = matches!(mode, AnswerMode::ProviderLlm);
    let claims_body = if matches!(mode, AnswerMode::ProviderLlm) {
        render_answer_from_claims(&structured)
    } else {
        // Extractive body without server notes for claim revalidation.
        let filtered: Vec<GroundingPassage> = passages_for_mode(&passages, &request.mode)
            .into_iter()
            .cloned()
            .collect();
        extractive_answer(&request.question, &filtered)
    };
    revalidate_final_answer(FinalAnswerCheck {
        answer: &claims_body,
        passages: &passages,
        mode: &request.mode,
        conflicts: &conflicts,
        structured: &structured,
        require_structured: require_structured_final,
        returned_citations: &resolved,
        claim_cite_ids: &claim_cite_ids,
        server_note_pin_cites: &server_note_pin_cites,
    })?;

    let grounded = !resolved.is_empty();
    let answer = finish_answer(BuildAnswer {
        answer,
        citations: resolved,
        mode,
        grounded,
        warnings,
        version_context,
        conflict_warnings,
        provider_configured,
        fallback_reason,
        request_id: &request_id,
    });
    // Non-stream: final guard + exact recheck before HTTP response.
    let colls: Vec<Uuid> = collection_ids.clone();
    let (guard, snapshot) =
        acquire_final_delivery_guard(lock_pool, pool, ctx, &answer, &colls).await?;
    GuardedQaResponse::from_answer(answer, guard, snapshot)
}

fn hydrated_to_hit(row: &search::HydratedChunkRow) -> RetrievalHit {
    let heading = row
        .heading_path
        .last()
        .cloned()
        .unwrap_or_else(|| "Mục".into());
    let span_start = row.span_start.unwrap_or(0).max(0) as usize;
    let span_end = row
        .span_end
        .map(|v| v.max(0) as usize)
        .unwrap_or_else(|| row.body.len());
    RetrievalHit {
        chunk_id: row.chunk_id,
        chunk_identity_sha256: row.chunk_identity_sha256.clone(),
        collection_id: row.collection_id,
        document_id: row.document_id,
        version_id: row.version_id,
        version_number: row.version_number,
        content_sha256: row.content_sha256.clone(),
        heading,
        snippet: row.body.clone(),
        body: row.body.clone(),
        lexical_score: 1.0,
        vector_score: 0.0,
        rerank_score: 1.0,
        is_current: row.is_current,
        effective_from: row.effective_from,
        effective_to: row.effective_to,
        page: row.page.map(|v| v as u32),
        slide: row.slide.map(|v| v as u32),
        sheet: row.sheet.clone(),
        span_start,
        span_end,
    }
}

fn representative_to_hits(rows: &[search::RepresentativeChunkRow]) -> Vec<RetrievalHit> {
    rows.iter()
        .map(|row| {
            let heading = row
                .heading_path
                .last()
                .cloned()
                .unwrap_or_else(|| "Mục".into());
            let span_start = row.span_start.unwrap_or(0).max(0) as usize;
            let span_end = row
                .span_end
                .map(|v| v.max(0) as usize)
                .unwrap_or_else(|| row.body.len());
            RetrievalHit {
                chunk_id: row.chunk_id,
                chunk_identity_sha256: row.chunk_identity_sha256.clone(),
                collection_id: row.collection_id,
                document_id: row.document_id,
                version_id: row.version_id,
                version_number: row.version_number,
                content_sha256: row.content_sha256.clone(),
                heading,
                snippet: row.body.clone(),
                body: row.body.clone(),
                lexical_score: 1.0,
                vector_score: 0.0,
                rerank_score: 1.0,
                is_current: row.is_current,
                effective_from: row.effective_from,
                effective_to: row.effective_to,
                page: row.page.map(|v| v as u32),
                slide: row.slide.map(|v| v as u32),
                sheet: row.sheet.clone(),
                span_start,
                span_end,
            }
        })
        .collect()
}

/// Bundled hermetic inputs to keep the public helper arity small.
pub struct HermeticAskInput<'a, P: QaChatProvider> {
    pub ctx: &'a OrgContext,
    pub request: QaRequest,
    pub retrieval: RetrievalResponse,
    pub passages: Vec<GroundingPassage>,
    pub conflicts: Vec<QaConflict>,
    pub timeline: Vec<VersionTimelineRow>,
    pub provider: Option<&'a P>,
    pub provider_config: Option<&'a QaProviderConfig>,
}

/// Hermetic ask helper for unit tests (no pool). Citations must be pre-attached.
pub async fn answer_question_hermetic<P: QaChatProvider>(
    input: HermeticAskInput<'_, P>,
) -> Result<QaAnswer, QaError> {
    let HermeticAskInput {
        ctx,
        request,
        retrieval,
        mut passages,
        conflicts,
        timeline,
        provider,
        provider_config,
    } = input;
    let request_id = new_request_id();
    if !ctx.has_permission(PERMISSION_QA_QUERY) {
        return Err(QaError::PermissionDenied);
    }
    bind_retrieval_provenance(ctx, &request, &retrieval)?;
    enforce_bounds(&request.question, &retrieval.hits)?;
    if passages.iter().any(|p| p.authoritative_quote.is_none()) {
        return Err(QaError::Citation);
    }
    if passages.is_empty() {
        return Ok(finish_answer(BuildAnswer {
            answer: extractive_answer(&request.question, &[]),
            citations: vec![],
            mode: AnswerMode::OfflineExtractive,
            grounded: false,
            warnings: vec!["Không có bằng chứng được ủy quyền.".into()],
            version_context: VersionContext {
                mode: version_mode_label(&request.mode),
                current_version_ids: vec![],
                cited_version_ids: vec![],
                change_note: None,
                history: vec![],
                history_page: None,
            },
            conflict_warnings: vec![],
            provider_configured: provider_config.is_some(),
            fallback_reason: Some("empty_evidence"),
            request_id: &request_id,
        }));
    }
    if let VersionMode::Compare {
        version_a,
        version_b,
        ..
    } = &request.mode
    {
        passages = filter_compare_passages(&passages, *version_a, *version_b)?;
    }
    let typed_delta = if let VersionMode::Compare {
        version_a,
        version_b,
        ..
    } = &request.mode
    {
        let typed: Vec<TypedVersionClaim> = conflicts
            .iter()
            .flat_map(|c| {
                [
                    TypedVersionClaim {
                        version_id: c.claim_a_version_id,
                        version_number: passages
                            .iter()
                            .find(|p| p.hit.version_id == c.claim_a_version_id)
                            .map(|p| p.hit.version_number)
                            .unwrap_or(0),
                        claim_key: c.claim_a_key.clone(),
                        scope: c.claim_a_scope.clone(),
                        unit: c.claim_a_unit.clone(),
                        value: c.claim_a_numeric.unwrap_or_default(),
                        chunk_id: passages
                            .iter()
                            .find(|p| p.hit.version_id == c.claim_a_version_id)
                            .map(|p| p.hit.chunk_id),
                        cite_id: Some(c.claim_a_cite_id.clone()),
                    },
                    TypedVersionClaim {
                        version_id: c.claim_b_version_id,
                        version_number: passages
                            .iter()
                            .find(|p| p.hit.version_id == c.claim_b_version_id)
                            .map(|p| p.hit.version_number)
                            .unwrap_or(0),
                        claim_key: c.claim_b_key.clone(),
                        scope: c.claim_b_scope.clone(),
                        unit: c.claim_b_unit.clone(),
                        value: c.claim_b_numeric.unwrap_or_default(),
                        chunk_id: passages
                            .iter()
                            .find(|p| p.hit.version_id == c.claim_b_version_id)
                            .map(|p| p.hit.chunk_id),
                        cite_id: Some(c.claim_b_cite_id.clone()),
                    },
                ]
            })
            .filter(|c| c.value != rust_decimal::Decimal::ZERO || c.cite_id.is_some())
            .collect();
        typed_numeric_delta_for_versions(*version_a, *version_b, &typed)
    } else {
        None
    };
    let prompt_passages = prompt_passages(&passages);
    let user = grounded_user_prompt(&request.question, &prompt_passages);
    let provider_configured = provider_config.is_some();
    let mut warnings = retrieval.warnings;
    let (raw_answer, mode, structured, fallback_reason, require_structured) = if !request.use_llm
        || provider.is_none()
    {
        let reason = if provider.is_none() && request.use_llm {
            warnings.push("QA provider unavailable; fallback extractive.".into());
            Some("provider_unavailable")
        } else {
            None
        };
        let mode = if reason.is_some() {
            AnswerMode::FallbackExtractive
        } else {
            AnswerMode::OfflineExtractive
        };
        let structured = extractive_structured_claims(&passages);
        (
            extractive_answer(&request.question, &passages),
            mode,
            structured,
            reason,
            false,
        )
    } else {
        let provider = provider.unwrap();
        let chat_request = ChatCompletionRequest {
            system: GROUNDED_SYSTEM_POLICY.to_string(),
            user,
        };
        let timeout_budget = provider_config
            .map(QaProviderConfig::timeout)
            .unwrap_or(Duration::from_secs(30));
        match tokio::time::timeout(timeout_budget, provider.complete_grounded(&chat_request)).await
        {
            Ok(Ok(payload)) if !payload.refusal => (
                render_answer_from_claims(&payload.claims),
                AnswerMode::ProviderLlm,
                payload.claims,
                None,
                true,
            ),
            Ok(Ok(_)) => {
                warnings.push("QA provider refused; fallback extractive.".into());
                let structured = extractive_structured_claims(&passages);
                (
                    extractive_answer(&request.question, &passages),
                    AnswerMode::FallbackExtractive,
                    structured,
                    Some("provider_refusal"),
                    false,
                )
            }
            Ok(Err(ProviderError::Timeout)) | Err(_) => {
                warnings.push("QA provider timeout; fallback extractive.".into());
                let structured = extractive_structured_claims(&passages);
                (
                    extractive_answer(&request.question, &passages),
                    AnswerMode::FallbackExtractive,
                    structured,
                    Some("provider_timeout"),
                    false,
                )
            }
            Ok(Err(_)) => {
                warnings.push("QA provider outage; fallback extractive.".into());
                let structured = extractive_structured_claims(&passages);
                (
                    extractive_answer(&request.question, &passages),
                    AnswerMode::FallbackExtractive,
                    structured,
                    Some("provider_outage"),
                    false,
                )
            }
        }
    };
    let (mut answer, cited, mode, structured, fallback_reason) = match validate_grounded_answer(
        &raw_answer,
        &passages,
        &request.mode,
        &conflicts,
        &structured,
        require_structured,
    ) {
        Ok(cited) => (raw_answer, cited, mode, structured, fallback_reason),
        Err(error) if matches!(mode, AnswerMode::ProviderLlm) => {
            warnings.push(format!(
                "Grounding không hợp lệ ({}); fallback extractive.",
                error.code()
            ));
            let answer = extractive_answer(&request.question, &passages);
            let cited: Vec<String> = passages.iter().map(|p| p.cite_id.clone()).collect();
            let structured = extractive_structured_claims(&passages);
            (
                answer,
                cited,
                AnswerMode::FallbackExtractive,
                structured,
                Some(error.code()),
            )
        }
        Err(error) => return Err(QaError::Grounding(error)),
    };
    let version_context = build_version_context(
        &request.mode,
        &passages,
        &cited,
        &timeline,
        typed_delta.as_ref(),
        None,
        None,
    )?;
    if let Some(note) = version_context.change_note.clone() {
        if !answer.contains("Thay đổi:") {
            answer.push_str("\n\n");
            answer.push_str(&note);
        }
    }
    let conflict_warnings = conflict_messages(&request.mode, &conflicts);
    let claim_cite_ids = cited.clone();
    let mut hydrate_cites = cited;
    for warning in &conflict_warnings {
        for pin in &warning.pin_cite_ids {
            if !hydrate_cites.iter().any(|c| c == pin) {
                hydrate_cites.push(pin.clone());
            }
        }
        if !answer.contains(&warning.message) {
            warnings.push(warning.message.clone());
        }
    }
    let server_note_pin_cites: Vec<String> = conflict_warnings
        .iter()
        .flat_map(|w| w.pin_cite_ids.clone())
        .collect();
    let by_id = GroundingPassage::by_cite_id(&passages);
    let citations: Vec<StableCitation> = hydrate_cites
        .iter()
        .filter_map(|id| {
            let passage = by_id.get(id)?;
            let quote = passage.authoritative_quote.clone()?;
            Some(StableCitation {
                org_id: ctx.org_id(),
                logical_document_id: passage.hit.document_id,
                version_id: passage.hit.version_id,
                version_number: passage.hit.version_number,
                content_sha256: passage.hit.content_sha256.clone(),
                chunk_id: passage.hit.chunk_id,
                chunk_identity_sha256: passage.hit.chunk_identity_sha256.clone(),
                page: passage.hit.page,
                slide: passage.hit.slide,
                sheet: passage.hit.sheet.clone(),
                span_start: passage.hit.span_start,
                span_end: passage.hit.span_end,
                quote,
                effective_from: passage.hit.effective_from,
                effective_to: passage.hit.effective_to,
                is_current: passage.hit.is_current,
                heading: passage.hit.heading.clone(),
            })
        })
        .collect();
    let claims_body = if matches!(mode, AnswerMode::ProviderLlm) {
        render_answer_from_claims(&structured)
    } else {
        extractive_answer(&request.question, &passages)
    };
    revalidate_final_answer(FinalAnswerCheck {
        answer: &claims_body,
        passages: &passages,
        mode: &request.mode,
        conflicts: &conflicts,
        structured: &structured,
        require_structured: matches!(mode, AnswerMode::ProviderLlm),
        returned_citations: &citations,
        claim_cite_ids: &claim_cite_ids,
        server_note_pin_cites: &server_note_pin_cites,
    })?;
    Ok(finish_answer(BuildAnswer {
        answer,
        citations,
        mode,
        grounded: !hydrate_cites.is_empty(),
        warnings,
        version_context,
        conflict_warnings,
        provider_configured,
        fallback_reason,
        request_id: &request_id,
    }))
}

/// Bundled hermetic stream inputs (ask + cancel/authz/bounds).
pub struct StreamHermeticInput<'a, P: QaChatProvider> {
    pub ask: HermeticAskInput<'a, P>,
    pub cancel: StreamCancel,
    pub authz: Option<AuthzWatch>,
    pub bounds: StreamBounds,
}

/// Hermetic stream helper (tests). Prefer [`stream_answer_live`] in production.
pub async fn stream_answer_hermetic<P: QaChatProvider>(
    input: StreamHermeticInput<'_, P>,
) -> Result<ProtectedStreamSession, QaError> {
    let StreamHermeticInput {
        ask,
        cancel,
        authz,
        bounds,
    } = input;
    let answer = answer_question_hermetic(ask).await?;
    let fence = authz
        .as_ref()
        .map(|w| w.fence().clone())
        .unwrap_or_else(|| {
            let f = AuthzEpochFence::new();
            f.capture(1);
            f
        });
    let tokens = tokenize_for_stream(&answer.answer)
        .into_iter()
        .map(Ok::<_, ()>);
    let raw = run_bounded_stream(
        futures::stream::iter(tokens),
        bounds,
        cancel.clone(),
        Some(fence.clone()),
    )
    .await;
    let metadata_json = Bytes::from(
        serde_json::to_vec(&qa_answer_envelope_json(&answer)).map_err(|_| QaError::Database)?,
    );
    Ok(ProtectedStreamSession {
        citations: answer.citations,
        warnings: answer.warnings,
        conflict_warnings: answer.conflict_warnings,
        version_context: answer.version_context,
        audit: answer.audit,
        grounded: answer.grounded,
        mode: answer.mode,
        metadata_json,
        receiver: ProtectedStreamReceiver::new(raw, Some(fence), cancel),
        session_guard: None,
    })
}

/// Inputs for the live streaming service entry.
pub struct StreamLiveInput<'a, S: BlobStore, P: QaChatProvider> {
    pub pool: &'a Pool,
    pub lock_pool: &'a LockPool,
    pub storage: &'a S,
    pub ctx: &'a OrgContext,
    pub request: QaRequest,
    pub retrieval: RetrievalResponse,
    pub provider: Option<&'a P>,
    pub provider_config: Option<&'a QaProviderConfig>,
    pub cancel: StreamCancel,
    pub bounds: StreamBounds,
}

/// Live streaming service entry: validates a grounded answer, then protected-replays
/// tokens under an authorization-epoch fence (initial full auth synchronous).
///
/// LLM answers are produced via cancellable provider SSE accumulate → validate,
/// then this entry protected-replays the validated body (no unused fake stream).
///
/// Returns session metadata + receiver — never a full-answer bypass beside the stream.
pub async fn stream_answer_live<S: BlobStore, P: QaChatProvider>(
    input: StreamLiveInput<'_, S, P>,
) -> Result<ProtectedStreamSession, QaError> {
    let StreamLiveInput {
        pool,
        lock_pool,
        storage,
        ctx,
        request,
        retrieval,
        provider,
        provider_config,
        cancel,
        bounds,
    } = input;
    let guarded = answer_question_live(
        pool,
        lock_pool,
        storage,
        ctx,
        request,
        retrieval,
        provider,
        provider_config,
        Some(&cancel),
    )
    .await?;
    let (answer, snapshot) = guarded.into_answer_for_stream(pool, ctx).await?;
    finish_protected_live_stream(pool, lock_pool, ctx, answer, snapshot, cancel, bounds).await
}

/// Accumulate provider SSE deltas under cancellation into a bounded structured payload.
pub async fn accumulate_provider_sse<P: QaChatProvider>(
    provider: &P,
    request: &ChatCompletionRequest,
    cancel: Option<&StreamCancel>,
    provider_config: Option<&QaProviderConfig>,
) -> Result<crate::services::qa::grounding::ProviderGroundedPayload, ProviderError> {
    use futures::StreamExt;
    let timeout_budget = provider_config
        .map(QaProviderConfig::timeout)
        .unwrap_or(Duration::from_secs(30));
    let mut acc = String::new();
    let mut stream = provider.stream_tokens_cancellable(request, cancel);
    let deadline = tokio::time::Instant::now() + timeout_budget;
    loop {
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(ProviderError::Cancelled);
        }
        let next = tokio::select! {
            biased;
            item = stream.next() => item,
            _ = async {
                if let Some(c) = cancel {
                    c.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                return Err(ProviderError::Cancelled);
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(ProviderError::Timeout);
            }
        };
        match next {
            Some(Ok(delta)) => {
                if acc.len().saturating_add(delta.len()) > MAX_RESPONSE_BYTES {
                    return Err(ProviderError::Truncated);
                }
                acc.push_str(&delta);
            }
            Some(Err(err)) => return Err(err),
            None => break,
        }
    }
    parse_grounded_payload(&acc)
}

async fn finish_protected_live_stream(
    pool: &Pool,
    lock_pool: &LockPool,
    ctx: &OrgContext,
    answer: QaAnswer,
    snapshot: ValidatedDeliverySnapshot,
    cancel: StreamCancel,
    bounds: StreamBounds,
) -> Result<ProtectedStreamSession, QaError> {
    // Carry validated pins from the final JSON guard into the stream session.
    let doc_ids = snapshot.document_ids.clone();
    let version_ids = snapshot.version_ids.clone();
    let collection_ids = snapshot.collection_ids.clone();
    let require_history = snapshot.require_history;
    let captured_epoch = snapshot.captured_epoch;
    let org_id = ctx.org_id();
    let user_id = ctx.user_id();
    let probe_ctx = ctx.clone();
    // Re-bind fence to the already-validated epoch (handoff already rechecked).
    let fence = AuthzEpochFence::new();
    fence.capture(captured_epoch);

    let pool_watch = pool.clone();
    let fence_watch = fence.clone();
    let doc_ids_watch = doc_ids.clone();
    // Watcher rereads epochs + exact mode/collections/versions/perms/ACL each tick (H6).
    let version_ids_watch = version_ids.clone();
    let collection_ids_watch = collection_ids.clone();
    let require_history_watch = require_history;
    spawn_epoch_watch(cancel.clone(), fence_watch, move || {
        let pool = pool_watch.clone();
        let doc_ids = doc_ids_watch.clone();
        let version_ids = version_ids_watch.clone();
        let collection_ids = collection_ids_watch.clone();
        let probe_ctx = probe_ctx.clone();
        async move {
            match with_org_txn(&pool, &probe_ctx, {
                let doc_ids = doc_ids.clone();
                let version_ids = version_ids.clone();
                let collection_ids = collection_ids.clone();
                move |txn| {
                    Box::pin(async move {
                        let probe = search::probe_stream_authz_exact(
                            txn,
                            org_id,
                            user_id,
                            &doc_ids,
                            &version_ids,
                            &collection_ids,
                            require_history_watch,
                        )
                        .await?;
                        if !matches!(probe, search::StreamAuthzProbe::Allow) {
                            return Ok::<_, crate::db::error::DbError>(match probe {
                                search::StreamAuthzProbe::Deleted => EpochWatchResult::Deleted,
                                _ => EpochWatchResult::Revoked,
                            });
                        }
                        let snap = authz_epoch::read_epoch_snapshot(txn, org_id, user_id, &doc_ids)
                            .await?;
                        Ok(EpochWatchResult::Allow {
                            composite_epoch: snap.composite(),
                        })
                    })
                }
            })
            .await
            {
                Ok(result) => result,
                Err(_) => EpochWatchResult::Revoked,
            }
        }
    });

    let tokens = tokenize_for_stream(&answer.answer)
        .into_iter()
        .map(Ok::<_, ()>);
    let raw = run_bounded_stream(
        futures::stream::iter(tokens),
        bounds,
        cancel.clone(),
        Some(fence.clone()),
    )
    .await;
    let lock_ctx = DeliveryLockContext {
        lock_pool: lock_pool.clone(),
        org_id,
        user_id,
        document_ids: doc_ids,
        version_ids,
        collection_ids,
        require_history,
        captured_epoch,
    };
    let session_guard = crate::services::qa::stream::acquire_session_guard(&lock_ctx, Some(&fence))
        .await
        .map_err(|_| QaError::PermissionDenied)?;
    let metadata_json = Bytes::from(
        serde_json::to_vec(&qa_answer_envelope_json(&answer)).map_err(|_| QaError::Database)?,
    );
    let receiver = ProtectedStreamReceiver::new(raw, Some(fence), cancel);
    Ok(ProtectedStreamSession {
        citations: answer.citations,
        warnings: answer.warnings,
        conflict_warnings: answer.conflict_warnings,
        version_context: answer.version_context,
        audit: answer.audit,
        grounded: answer.grounded,
        mode: answer.mode,
        metadata_json,
        receiver,
        session_guard: Some(session_guard),
    })
}

/// Membership revoke mutation helper: exclusive advisory lock, bump epoch, delete.
pub async fn revoke_membership_with_fence(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    fence: &AuthzEpochFence,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<(), QaError> {
    fence
        .revoke_and_drain(crate::services::qa::authz_fence::CloseKind::Revoked)
        .await;
    authz_mutation::revoke_membership(lock_pool, ctx, org_id, user_id)
        .await
        .map_err(|err| match err {
            authz_mutation::AuthzMutationError::PermissionDenied => QaError::PermissionDenied,
            authz_mutation::AuthzMutationError::LockTimeout => QaError::Database,
            _ => QaError::Database,
        })?;
    Ok(())
}

pub fn retrieval_fixture(
    ctx: &OrgContext,
    mode: VersionMode,
    hits: Vec<RetrievalHit>,
    warnings: Vec<String>,
) -> RetrievalResponse {
    RetrievalResponse {
        hits,
        warnings,
        embedding_mode: "test".into(),
        conflict_evidence: vec![],
        vector_weight: fileconv_knowledge::rank::VECTOR_WEIGHT,
        provenance: RetrievalProvenance {
            org_id: ctx.org_id(),
            user_id: ctx.user_id(),
            mode,
            collection_ids: ctx.allowed_collection_ids().clone(),
            retrieved_at: Utc::now(),
            document_id: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::qa::grounding::{ProviderGroundedPayload, StructuredClaim};
    use crate::services::qa::provider::{HangingProvider, ScriptedProvider};
    use chrono::TimeZone;
    use rust_decimal::Decimal;

    fn ctx() -> OrgContext {
        OrgContext::try_new(
            Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap(),
            [PERMISSION_QA_QUERY, PERMISSION_QA_HISTORY],
            [Uuid::parse_str("55555555-5555-5555-5555-555555555501").unwrap()],
        )
        .unwrap()
    }

    fn sample_hit(
        version_id: Uuid,
        version_number: i32,
        is_current: bool,
        snippet: &str,
    ) -> RetrievalHit {
        RetrievalHit {
            chunk_id: Uuid::new_v4(),
            chunk_identity_sha256: "c".repeat(64),
            collection_id: Uuid::parse_str("55555555-5555-5555-5555-555555555501").unwrap(),
            document_id: Uuid::parse_str("66666666-6666-6666-6666-666666666601").unwrap(),
            version_id,
            version_number,
            content_sha256: "d".repeat(64),
            heading: "Kinh phí".into(),
            snippet: snippet.into(),
            body: snippet.into(),
            lexical_score: 1.0,
            vector_score: 0.8,
            rerank_score: 1.8,
            is_current,
            effective_from: Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap(),
            effective_to: None,
            page: Some(2),
            slide: None,
            sheet: None,
            span_start: 0,
            span_end: snippet.len(),
        }
    }

    fn passages_from(hits: &[RetrievalHit]) -> Vec<GroundingPassage> {
        GroundingPassage::from_hits(hits)
            .into_iter()
            .map(|mut p| {
                p.authoritative_quote = Some(p.hit.snippet.clone());
                p
            })
            .collect()
    }

    #[tokio::test]
    async fn empty_evidence_is_ungrounded() {
        let ctx = ctx();
        let request = QaRequest {
            question: "Kinh phí?".into(),
            mode: VersionMode::Current,
            use_llm: false,
            collection_ids: None,
        };
        let retrieval = retrieval_fixture(&ctx, VersionMode::Current, vec![], vec![]);
        let answer = answer_question_hermetic::<ScriptedProvider>(HermeticAskInput {
            ctx: &ctx,
            request,
            retrieval,
            passages: vec![],
            conflicts: vec![],
            timeline: vec![],
            provider: None,
            provider_config: None,
        })
        .await
        .unwrap();
        assert!(!answer.grounded);
        assert!(answer.citations.is_empty());
    }

    #[tokio::test]
    async fn fabricated_and_malformed_citations_fallback() {
        let ctx = ctx();
        let v2 = Uuid::parse_str("77777777-7777-7777-7777-777777777702").unwrap();
        let quote = "Kinh phí phê duyệt là 15 triệu đồng.";
        let hits = vec![sample_hit(v2, 2, true, quote)];
        let retrieval = retrieval_fixture(&ctx, VersionMode::Current, hits.clone(), vec![]);
        let passages = passages_from(&hits);
        let provider = ScriptedProvider {
            result: Ok(ProviderGroundedPayload {
                claims: vec![StructuredClaim {
                    text: "Bịa đặt.".into(),
                    cite_ids: vec!["CITE-9999".into()],
                    kind: None,
                    value: None,
                    unit: None,
                }],
                refusal: false,
            }),
            chunks: vec![],
        };
        let request = QaRequest {
            question: "Kinh phí hiện tại là bao nhiêu?".into(),
            mode: VersionMode::Current,
            use_llm: true,
            collection_ids: None,
        };
        let answer = answer_question_hermetic(HermeticAskInput {
            ctx: &ctx,
            request,
            retrieval,
            passages,
            conflicts: vec![],
            timeline: vec![],
            provider: Some(&provider),
            provider_config: None,
        })
        .await
        .unwrap();
        assert_eq!(answer.mode, AnswerMode::FallbackExtractive);
        assert!(answer.answer.contains("[CITE-0001]"));
    }

    #[tokio::test]
    async fn stale_provenance_is_denied() {
        let ctx = ctx();
        let mut retrieval = retrieval_fixture(&ctx, VersionMode::Current, vec![], vec![]);
        retrieval.provenance.user_id = Uuid::new_v4();
        let err = answer_question_hermetic::<ScriptedProvider>(HermeticAskInput {
            ctx: &ctx,
            request: QaRequest {
                question: "q?".into(),
                mode: VersionMode::Current,
                use_llm: false,
                collection_ids: None,
            },
            retrieval,
            passages: vec![],
            conflicts: vec![],
            timeline: vec![],
            provider: None,
            provider_config: None,
        })
        .await
        .unwrap_err();
        assert_eq!(err, QaError::StaleRetrieval);
    }

    #[tokio::test]
    async fn provider_timeout_falls_back_extractive() {
        let ctx = ctx();
        let v2 = Uuid::parse_str("77777777-7777-7777-7777-777777777702").unwrap();
        let quote = "Kinh phí phê duyệt là 15 triệu đồng.";
        let hits = vec![sample_hit(v2, 2, true, quote)];
        let retrieval = retrieval_fixture(&ctx, VersionMode::Current, hits.clone(), vec![]);
        let passages = passages_from(&hits);
        let hanging = HangingProvider {
            delay: Duration::from_secs(60),
        };
        let config = QaProviderConfig::with_api_key(
            "http://127.0.0.1:9/v1",
            "key-not-fake",
            "configured-model",
            "glm",
            Duration::from_millis(30),
            [] as [&str; 0],
            true,
            crate::config::Profile::Dev,
        )
        .unwrap();
        let answer = answer_question_hermetic(HermeticAskInput {
            ctx: &ctx,
            request: QaRequest {
                question: "Kinh phí hiện tại là bao nhiêu?".into(),
                mode: VersionMode::Current,
                use_llm: true,
                collection_ids: None,
            },
            retrieval,
            passages,
            conflicts: vec![],
            timeline: vec![],
            provider: Some(&hanging),
            provider_config: Some(&config),
        })
        .await
        .unwrap();
        assert_eq!(answer.mode, AnswerMode::FallbackExtractive);
        assert_eq!(answer.audit.fallback_reason, Some("provider_timeout"));
    }

    #[tokio::test]
    async fn stream_revoke_delivers_no_buffered_tail() {
        let ctx = ctx();
        let v2 = Uuid::parse_str("77777777-7777-7777-7777-777777777702").unwrap();
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu ";
        let hits = vec![sample_hit(v2, 2, true, long)];
        let retrieval = retrieval_fixture(&ctx, VersionMode::Current, hits.clone(), vec![]);
        let passages = passages_from(&hits);
        let watch = AuthzWatch::new();
        let watch_signal = watch.clone();
        let bounds = StreamBounds {
            max_tokens: 1_000,
            max_bytes: 64 * 1024,
            buffer: 16,
            backpressure_wait: Duration::from_secs(1),
            overall_timeout: Some(Duration::from_secs(5)),
            source_wait: Duration::from_secs(1),
        };
        let rx = stream_answer_hermetic::<ScriptedProvider>(StreamHermeticInput {
            ask: HermeticAskInput {
                ctx: &ctx,
                request: QaRequest {
                    question: "Nội dung dài để stream nhiều token có căn cứ rõ ràng?".into(),
                    mode: VersionMode::Current,
                    use_llm: false,
                    collection_ids: None,
                },
                retrieval,
                passages,
                conflicts: vec![],
                timeline: vec![],
                provider: None,
                provider_config: None,
            },
            cancel: StreamCancel::new(),
            authz: Some(watch),
            bounds,
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        watch_signal.signal_revoked();
        let (body, reason) = collect_sse_token_text(rx.into_sse_body()).await;
        assert_eq!(reason, StreamCloseReason::AuthzRevoked);
        assert!(!body.contains("lambda mu nu"));
    }

    #[test]
    fn audit_and_debug_redact_content() {
        let request = QaRequest {
            question: "secret question".into(),
            mode: VersionMode::Current,
            use_llm: false,
            collection_ids: None,
        };
        assert!(!format!("{request:?}").contains("secret question"));
        let meta = QaAuditMetadata {
            action: "qa.ask",
            outcome: "fallback",
            answer_mode: "fallback_extractive",
            citation_count: 0,
            conflict_warning_count: 0,
            version_mode: "current",
            provider_configured: false,
            fallback_reason: Some("provider_outage"),
            request_id: new_request_id(),
            grounded: false,
        };
        assert!(!meta.to_json().to_string().contains("secret"));
        assert_eq!(meta.request_id.len(), 32);
    }

    #[tokio::test]
    async fn chunked_sse_accumulate_validates_structured_payload() {
        let payload = ProviderGroundedPayload {
            claims: vec![StructuredClaim {
                text: "Kinh phí phê duyệt là 15 triệu đồng.".into(),
                cite_ids: vec!["CITE-0001".into()],
                kind: Some("numeric".into()),
                value: Some("15".into()),
                unit: Some("triệu".into()),
            }],
            refusal: false,
        };
        let encoded = serde_json::to_string(&payload).unwrap();
        let mid = encoded.len() / 2;
        let provider = ScriptedProvider {
            result: Ok(payload.clone()),
            chunks: vec![encoded[..mid].into(), encoded[mid..].into()],
        };
        let chat = ChatCompletionRequest {
            system: "sys".into(),
            user: "user".into(),
        };
        let got = accumulate_provider_sse(&provider, &chat, None, None)
            .await
            .unwrap();
        assert_eq!(got, payload);
    }

    #[test]
    fn typed_delta_orders_by_version_number() {
        let v1 = Uuid::parse_str("77777777-7777-7777-7777-777777777701").unwrap();
        let v2 = Uuid::parse_str("77777777-7777-7777-7777-777777777702").unwrap();
        let c1 = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa1").unwrap();
        let c2 = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaa2").unwrap();
        let claims = vec![
            TypedVersionClaim {
                version_id: v2,
                version_number: 2,
                claim_key: "budget".into(),
                scope: "org".into(),
                unit: Some("VND".into()),
                value: Decimal::from(15),
                chunk_id: Some(c2),
                cite_id: Some("CITE-0002".into()),
            },
            TypedVersionClaim {
                version_id: v1,
                version_number: 1,
                claim_key: "budget".into(),
                scope: "org".into(),
                unit: Some("VND".into()),
                value: Decimal::from(10),
                chunk_id: Some(c1),
                cite_id: Some("CITE-0001".into()),
            },
        ];
        let delta = grounding::typed_numeric_delta_for_versions(v1, v2, &claims).unwrap();
        assert_eq!(delta.delta, Decimal::from(5));
        assert_eq!(
            (delta.older_version_number, delta.newer_version_number),
            (1, 2)
        );
    }
}
