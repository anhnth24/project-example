//! Safe, append-only business audit helpers (P1B-O01 / Sol #6).
//!
//! Typed actions + per-action scalar metadata allowlists. Never document content,
//! prompts, tokens, passwords, or signed URLs. Mutation paths must write audit in
//! the same DB transaction as the business change (or fail the mutation).

use deadpool_postgres::Pool;
use serde_json::{json, Value as JsonValue};
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::session::{write_audit, AuditEvent};
use crate::db::error::DbError;
use crate::db::models::AuditOutcome;
use crate::db::pool::with_org_txn;
use crate::telemetry::{redacted_fields, AuditEvent as TelemetryAuditShape};

/// Stable audit action names used across routes/services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    AuthLogin,
    AuthDeny,
    AuthLogout,
    AuthRefresh,
    AuthRefreshReuse,
    AuthRevokeAll,
    CollectionCreate,
    CollectionUpdate,
    CollectionDelete,
    DocumentUpload,
    DocumentDelete,
    DocumentTombstone,
    DocumentPreview,
    DocumentPublish,
    DocumentReindex,
    DocumentPurge,
    DocumentPurgeObjects,
    DocumentAsk,
    DocumentAskStream,
    DocumentSearch,
    UploadApproveInfix,
    ConflictTriage,
    JobEnqueue,
    QuotaDeny,
    ReconcileRepair,
    ReconcileObjectCleanup,
    ReconcileDeadLetterGc,
    VectorCleanupIntent,
    ObjectCleanup,
}

impl AuditAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthLogin => "auth.login",
            Self::AuthDeny => "auth.deny",
            Self::AuthLogout => "auth.logout",
            Self::AuthRefresh => "auth.refresh",
            Self::AuthRefreshReuse => "auth.refresh.reuse",
            Self::AuthRevokeAll => "auth.revoke_all",
            Self::CollectionCreate => "collection.create",
            Self::CollectionUpdate => "collection.update",
            Self::CollectionDelete => "collection.delete",
            Self::DocumentUpload => "document.upload",
            Self::DocumentDelete => "document.delete",
            Self::DocumentTombstone => "document.tombstone",
            Self::DocumentPreview => "document.preview",
            Self::DocumentPublish => "document.publish",
            Self::DocumentReindex => "document.reindex",
            Self::DocumentPurge => "document.purge",
            Self::DocumentPurgeObjects => "document.purge_objects",
            Self::DocumentAsk => "ask.query",
            Self::DocumentAskStream => "ask.stream",
            Self::DocumentSearch => "search.query",
            Self::UploadApproveInfix => "upload.approve_intake",
            Self::ConflictTriage => "conflict.triage",
            Self::JobEnqueue => "job.enqueue",
            Self::QuotaDeny => "quota.deny",
            Self::ReconcileRepair => "reconcile.repair",
            Self::ReconcileObjectCleanup => "reconcile.object_cleanup",
            Self::ReconcileDeadLetterGc => "reconcile.dead_letter_gc",
            Self::VectorCleanupIntent => "vector.cleanup_intent",
            Self::ObjectCleanup => "object.cleanup",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auth.login" => Ok(Self::AuthLogin),
            "auth.deny" => Ok(Self::AuthDeny),
            "auth.logout" => Ok(Self::AuthLogout),
            "auth.refresh" => Ok(Self::AuthRefresh),
            "auth.refresh.reuse" => Ok(Self::AuthRefreshReuse),
            "auth.revoke_all" => Ok(Self::AuthRevokeAll),
            "collection.create" => Ok(Self::CollectionCreate),
            "collection.update" => Ok(Self::CollectionUpdate),
            "collection.delete" => Ok(Self::CollectionDelete),
            "document.upload" => Ok(Self::DocumentUpload),
            "document.delete" => Ok(Self::DocumentDelete),
            "document.tombstone" => Ok(Self::DocumentTombstone),
            "document.preview" => Ok(Self::DocumentPreview),
            "document.publish" => Ok(Self::DocumentPublish),
            "document.reindex" => Ok(Self::DocumentReindex),
            "document.purge" => Ok(Self::DocumentPurge),
            "document.purge_objects" => Ok(Self::DocumentPurgeObjects),
            "ask.query" => Ok(Self::DocumentAsk),
            "ask.stream" => Ok(Self::DocumentAskStream),
            "search.query" => Ok(Self::DocumentSearch),
            "upload.approve_intake" => Ok(Self::UploadApproveInfix),
            "conflict.triage" => Ok(Self::ConflictTriage),
            "job.enqueue" => Ok(Self::JobEnqueue),
            "quota.deny" => Ok(Self::QuotaDeny),
            "reconcile.repair" => Ok(Self::ReconcileRepair),
            "reconcile.object_cleanup" => Ok(Self::ReconcileObjectCleanup),
            "reconcile.dead_letter_gc" => Ok(Self::ReconcileDeadLetterGc),
            "vector.cleanup_intent" => Ok(Self::VectorCleanupIntent),
            "object.cleanup" => Ok(Self::ObjectCleanup),
            _ => Err(format!("audit_action_invalid:{value}")),
        }
    }

    /// Exact scalar metadata keys permitted for this action.
    pub fn metadata_keys(self) -> &'static [&'static str] {
        match self {
            Self::AuthLogin | Self::AuthLogout => {
                &["reason", "error_class", "family_id", "refresh_id"]
            }
            Self::AuthDeny | Self::AuthRevokeAll => &["reason", "error_class"],
            Self::AuthRefresh | Self::AuthRefreshReuse => &[
                "reason",
                "family_id",
                "token_id",
                "refresh_id",
                "replaced_id",
            ],
            // IDs / enums / counts / hashes only — never name/slug/filename/content free strings.
            Self::CollectionCreate | Self::CollectionUpdate | Self::CollectionDelete => {
                &["reason", "collection_id", "name_chars", "slug_chars"]
            }
            Self::DocumentUpload => &[
                "reason",
                "format",
                "document_id",
                "version_id",
                "collection_id",
                "byte_size",
                "content_sha256",
            ],
            Self::DocumentDelete | Self::DocumentTombstone => &[
                "reason",
                "document_id",
                "version_id",
                "cancelled_writer_jobs",
                "hard",
            ],
            Self::DocumentPreview => &["reason", "document_id", "version_id"],
            Self::DocumentPublish | Self::DocumentReindex => {
                &["reason", "document_id", "version_id", "job_id", "job_type"]
            }
            Self::DocumentPurge | Self::DocumentPurgeObjects => &[
                "document_id",
                "phase",
                "object_count",
                "deleted_chunks",
                "cancelled_writer_jobs",
                "job_id",
            ],
            Self::DocumentAsk => &[
                "reason",
                "mode",
                "citation_count",
                "question_chars",
                "stream",
            ],
            Self::DocumentAskStream => &["reason", "stream_session_id", "question_chars"],
            Self::DocumentSearch => &["reason", "hit_count", "query_chars", "limit"],
            Self::UploadApproveInfix => &[
                "reason",
                "collection_id",
                "job_id",
                "created",
                "document_id",
                "version_id",
            ],
            Self::ConflictTriage => &["reason", "status", "conflict_id"],
            Self::JobEnqueue => &["job_id", "job_type"],
            Self::QuotaDeny => &["reason", "resource_kind", "error_class"],
            Self::ReconcileRepair => &[
                "document_id",
                "phase",
                "orphan_vectors",
                "stale_vectors",
                "orphan_objects",
                "rebuilt_vector_jobs",
                "result",
            ],
            Self::ReconcileObjectCleanup | Self::ObjectCleanup => {
                &["document_id", "phase", "object_count", "result"]
            }
            Self::ReconcileDeadLetterGc => &[
                "document_id",
                "phase",
                "result",
                "checked_count",
                "object_count",
            ],
            Self::VectorCleanupIntent => &["document_id", "phase", "result", "point_count"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResource {
    Session,
    Document,
    Collection,
    Job,
    Quota,
    Object,
    Ask,
    AskStream,
    Search,
    Conflict,
}

impl AuditResource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Document => "document",
            Self::Collection => "collection",
            Self::Job => "job",
            Self::Quota => "quota",
            Self::Object => "object",
            Self::Ask => "ask",
            Self::AskStream => "ask_stream",
            Self::Search => "search",
            Self::Conflict => "conflict",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "session" => Ok(Self::Session),
            "document" => Ok(Self::Document),
            "collection" => Ok(Self::Collection),
            "job" | "jobs" => Ok(Self::Job),
            "quota" => Ok(Self::Quota),
            "object" => Ok(Self::Object),
            "ask" => Ok(Self::Ask),
            "ask_stream" => Ok(Self::AskStream),
            "search" => Ok(Self::Search),
            "conflict" => Ok(Self::Conflict),
            _ => Err(format!("audit_resource_type_invalid:{value}")),
        }
    }
}

/// Closed set of durable audit reason codes (no free text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditReason {
    UnknownUser,
    UserDisabled,
    BadPassword,
    PermissionDenied,
    MembershipMissing,
    CollectionDenied,
    InvalidCredentials,
    RefreshReuse,
    RefreshExpired,
    Expired,
    RefreshRace,
    UserRequested,
    UploadAccepted,
    QuotaExceeded,
    ValidationFailed,
    System,
    ProviderError,
    NotFound,
}

impl AuditReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownUser => "unknown_user",
            Self::UserDisabled => "user_disabled",
            Self::BadPassword => "bad_password",
            Self::PermissionDenied => "permission_denied",
            Self::MembershipMissing => "membership_missing",
            Self::CollectionDenied => "collection_denied",
            Self::InvalidCredentials => "invalid_credentials",
            Self::RefreshReuse => "refresh_reuse",
            Self::RefreshExpired => "refresh_expired",
            Self::Expired => "expired",
            Self::RefreshRace => "refresh_race",
            Self::UserRequested => "user_requested",
            Self::UploadAccepted => "upload_accepted",
            Self::QuotaExceeded => "quota_exceeded",
            Self::ValidationFailed => "validation_failed",
            Self::System => "system",
            Self::ProviderError => "provider_error",
            Self::NotFound => "not_found",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "unknown_user" => Ok(Self::UnknownUser),
            "user_disabled" => Ok(Self::UserDisabled),
            "bad_password" => Ok(Self::BadPassword),
            "permission_denied" => Ok(Self::PermissionDenied),
            "membership_missing" => Ok(Self::MembershipMissing),
            "collection_denied" => Ok(Self::CollectionDenied),
            "invalid_credentials" => Ok(Self::InvalidCredentials),
            "refresh_reuse" => Ok(Self::RefreshReuse),
            "refresh_expired" => Ok(Self::RefreshExpired),
            "expired" => Ok(Self::Expired),
            "refresh_race" => Ok(Self::RefreshRace),
            "user_requested" => Ok(Self::UserRequested),
            "upload_accepted" => Ok(Self::UploadAccepted),
            "quota_exceeded" => Ok(Self::QuotaExceeded),
            "validation_failed" => Ok(Self::ValidationFailed),
            "system" => Ok(Self::System),
            "provider_error" => Ok(Self::ProviderError),
            "not_found" => Ok(Self::NotFound),
            _ => Err(format!("audit_reason_invalid:{value}")),
        }
    }
}

/// Bundled fields for a durable audit write.
#[derive(Debug, Clone)]
pub struct AuditRecord<'a> {
    pub request_id: &'a str,
    pub action: &'a str,
    pub resource_type: &'a str,
    pub resource_id: Option<&'a str>,
    pub outcome: AuditOutcome,
    pub metadata: JsonValue,
}

/// Defense-in-depth: drop forbidden key names (tests / callers that only need
/// secret stripping). Durable writes must use action-scoped allowlists.
pub fn sanitize_metadata(metadata: JsonValue) -> JsonValue {
    let JsonValue::Object(map) = metadata else {
        return json!({});
    };
    let mut safe = serde_json::Map::new();
    for (key, value) in map {
        let lowered = key.to_ascii_lowercase();
        if FORBIDDEN_METADATA_KEYS
            .iter()
            .any(|forbidden| lowered.contains(forbidden))
        {
            continue;
        }
        match value {
            JsonValue::String(text) => {
                let mut fields = std::collections::BTreeMap::new();
                fields.insert(key.clone(), text);
                let redacted = redacted_fields(&fields);
                safe.insert(
                    key,
                    JsonValue::String(redacted.values().next().cloned().unwrap_or_default()),
                );
            }
            JsonValue::Number(_) | JsonValue::Bool(_) | JsonValue::Null => {
                safe.insert(key, value);
            }
            other => {
                let _ = other; // nested objects/arrays never persist
            }
        }
    }
    JsonValue::Object(safe)
}

const FORBIDDEN_METADATA_KEYS: &[&str] = &[
    "password",
    "token",
    "refresh_token",
    "access_token",
    "authorization",
    "prompt",
    "document_content",
    "markdown",
    "signed_url",
    "object_key",
    "answer",
    "question",
    "email",
    "api_key",
];

/// Per-action allowlist: unknown keys / nested values / free-text reasons fail.
pub fn sanitize_for_action(action: AuditAction, metadata: &JsonValue) -> Result<JsonValue, String> {
    let JsonValue::Object(map) = metadata else {
        return Err("audit_metadata_must_be_object".into());
    };
    let allowed = action.metadata_keys();
    let mut filtered = serde_json::Map::new();
    for (key, value) in map {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "audit_metadata_key_not_allowlisted_for_action:{key}"
            ));
        }
        match value {
            JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) => {
                filtered.insert(key.clone(), value.clone());
            }
            JsonValue::String(text) => {
                if key == "reason" {
                    AuditReason::parse(text)?;
                }
                // Reject nested canary-looking free text in non-reason strings by length.
                if text.len() > 128 {
                    return Err("audit_metadata_string_too_long".into());
                }
                filtered.insert(key.clone(), JsonValue::String(text.clone()));
            }
            _ => return Err("audit_metadata_value_must_be_scalar".into()),
        }
    }
    // Second pass: strip any forbidden fragments that slipped into string values.
    Ok(sanitize_metadata(JsonValue::Object(filtered)))
}

#[cfg(any(test, feature = "test-hooks"))]
static INJECT_AUDIT_FAILURE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Arms a one-shot failure for the next [`record_in_txn`] (test-hooks / unit tests only).
#[cfg(any(test, feature = "test-hooks"))]
pub fn arm_injected_audit_failure() {
    INJECT_AUDIT_FAILURE.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn require_request_uuid(request_id: &str) -> Result<String, DbError> {
    Uuid::parse_str(request_id.trim())
        .map(|id| id.to_string())
        .map_err(|_| DbError::Config("audit_request_id_must_be_uuid".into()))
}

/// Write an audit row inside an existing org transaction (mutation co-commit).
pub async fn record_in_txn(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    record: AuditRecord<'_>,
) -> Result<(), DbError> {
    #[cfg(any(test, feature = "test-hooks"))]
    if INJECT_AUDIT_FAILURE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Err(DbError::Config("injected_audit_failure".into()));
    }
    let action = AuditAction::parse(record.action).map_err(DbError::Config)?;
    let resource = AuditResource::parse(record.resource_type).map_err(DbError::Config)?;
    let request_id = require_request_uuid(record.request_id)?;
    let metadata = sanitize_for_action(action, &record.metadata).map_err(DbError::Config)?;
    write_audit(
        txn,
        AuditEvent {
            org_id: ctx.org_id(),
            actor_user_id: Some(ctx.user_id()),
            action: action.as_str(),
            resource_type: resource.as_str(),
            resource_id: record.resource_id,
            outcome: record.outcome.as_str(),
            metadata,
            request_id: &request_id,
        },
    )
    .await
}

/// Durable deny audit for permission/authorization failures (must not be swallowed).
pub async fn record_deny(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    action: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    reason: &str,
) -> Result<(), DbError> {
    let _ = AuditReason::parse(reason).map_err(DbError::Config)?;
    record(
        pool,
        ctx,
        AuditRecord {
            request_id,
            action,
            resource_type,
            resource_id,
            outcome: AuditOutcome::Deny,
            metadata: json!({ "reason": reason }),
        },
    )
    .await
}

/// Central typed HTTP route audit surface (success / deny / error).
///
/// Callers must not swallow required audits — propagate [`DbError`] so the
/// route fails closed when durable audit cannot be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteAuditKind {
    Upload,
    Ask,
    AskStream,
    Search,
    CollectionCreate,
    CollectionUpdate,
    CollectionDelete,
    DocumentGet,
    DocumentDelete,
    DocumentReindex,
}

impl RouteAuditKind {
    pub const fn action(self) -> AuditAction {
        match self {
            Self::Upload => AuditAction::DocumentUpload,
            Self::Ask => AuditAction::DocumentAsk,
            Self::AskStream => AuditAction::DocumentAskStream,
            Self::Search => AuditAction::DocumentSearch,
            Self::CollectionCreate => AuditAction::CollectionCreate,
            Self::CollectionUpdate => AuditAction::CollectionUpdate,
            Self::CollectionDelete => AuditAction::CollectionDelete,
            Self::DocumentGet => AuditAction::DocumentPreview,
            Self::DocumentDelete => AuditAction::DocumentDelete,
            Self::DocumentReindex => AuditAction::DocumentReindex,
        }
    }

    pub const fn resource(self) -> AuditResource {
        match self {
            Self::Upload | Self::DocumentGet | Self::DocumentDelete | Self::DocumentReindex => {
                AuditResource::Document
            }
            Self::Ask => AuditResource::Ask,
            Self::AskStream => AuditResource::AskStream,
            Self::Search => AuditResource::Search,
            Self::CollectionCreate | Self::CollectionUpdate | Self::CollectionDelete => {
                AuditResource::Collection
            }
        }
    }
}

/// Typed success/deny/error helper — never swallow; returns Err when write fails.
pub async fn record_route(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    kind: RouteAuditKind,
    outcome: AuditOutcome,
    resource_id: Option<&str>,
    metadata: JsonValue,
) -> Result<(), DbError> {
    record(
        pool,
        ctx,
        AuditRecord {
            request_id,
            action: kind.action().as_str(),
            resource_type: kind.resource().as_str(),
            resource_id,
            outcome,
            metadata,
        },
    )
    .await
}

/// Typed deny for a route (exact action/resource); must not be swallowed.
pub async fn record_route_deny(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    kind: RouteAuditKind,
    resource_id: Option<&str>,
    reason: AuditReason,
) -> Result<(), DbError> {
    record_route(
        pool,
        ctx,
        request_id,
        kind,
        AuditOutcome::Deny,
        resource_id,
        json!({ "reason": reason.as_str() }),
    )
    .await
}

/// Typed error for a route; must not be swallowed.
pub async fn record_route_error(
    pool: &Pool,
    ctx: &OrgContext,
    request_id: &str,
    kind: RouteAuditKind,
    resource_id: Option<&str>,
    reason: AuditReason,
) -> Result<(), DbError> {
    record_route(
        pool,
        ctx,
        request_id,
        kind,
        AuditOutcome::Error,
        resource_id,
        json!({ "reason": reason.as_str() }),
    )
    .await
}

pub async fn record(pool: &Pool, ctx: &OrgContext, record: AuditRecord<'_>) -> Result<(), DbError> {
    let action = AuditAction::parse(record.action).map_err(DbError::Config)?;
    let resource = AuditResource::parse(record.resource_type).map_err(DbError::Config)?;
    let request_id = require_request_uuid(record.request_id)?;
    let metadata = sanitize_for_action(action, &record.metadata).map_err(DbError::Config)?;
    let outcome = record.outcome.as_str().to_string();
    let resource_id = record.resource_id.map(str::to_string);
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        let action = action.as_str().to_string();
        let resource_type = resource.as_str().to_string();
        move |txn| {
            Box::pin(async move {
                write_audit(
                    txn,
                    AuditEvent {
                        org_id: ctx.org_id(),
                        actor_user_id: Some(ctx.user_id()),
                        action: &action,
                        resource_type: &resource_type,
                        resource_id: resource_id.as_deref(),
                        outcome: &outcome,
                        metadata,
                        request_id: &request_id,
                    },
                )
                .await
            })
        }
    })
    .await
}

#[derive(Debug, Clone)]
pub struct TelemetryAuditInput<'a> {
    pub org_id: Uuid,
    pub actor_id: Uuid,
    pub request_id: &'a str,
    pub action: &'a str,
    pub resource_type: &'a str,
    pub resource_id: &'a str,
    pub outcome: AuditOutcome,
    pub metadata: &'a [(String, String)],
}

/// Converts a durable audit row into the telemetry envelope (no secrets).
pub fn to_telemetry_envelope(input: TelemetryAuditInput<'_>) -> TelemetryAuditShape {
    let mut map = std::collections::BTreeMap::new();
    for (key, value) in input.metadata {
        map.insert(key.clone(), value.clone());
    }
    TelemetryAuditShape {
        version: 1,
        occurred_at: chrono::Utc::now().to_rfc3339(),
        request_id: input.request_id.into(),
        org_id: input.org_id.to_string(),
        actor_id: input.actor_id.to_string(),
        action: input.action.into(),
        target_type: input.resource_type.into(),
        target_id: input.resource_id.into(),
        outcome: input.outcome.as_str().into(),
        metadata: redacted_fields(&map),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_secrets_and_nested() {
        let cleaned = sanitize_metadata(json!({
            "reason": "ok",
            "password": "secret",
            "prompt": "leak",
            "object_key": "trusted/abc",
            "attempts": 2,
            "nested": {"x": 1}
        }));
        assert_eq!(cleaned["reason"], "ok");
        assert_eq!(cleaned["attempts"], 2);
        assert!(cleaned.get("password").is_none());
        assert!(cleaned.get("prompt").is_none());
        assert!(cleaned.get("object_key").is_none());
        assert!(cleaned.get("nested").is_none());
    }

    #[test]
    fn typed_outcomes_roundtrip_including_intent() {
        for outcome in [
            AuditOutcome::Success,
            AuditOutcome::Deny,
            AuditOutcome::Error,
            AuditOutcome::Intent,
        ] {
            assert_eq!(AuditOutcome::parse(outcome.as_str()).unwrap(), outcome);
        }
    }

    #[test]
    fn allowlist_rejects_free_reason_and_unknown_keys() {
        let action = AuditAction::DocumentDelete;
        assert!(sanitize_for_action(action, &json!({ "reason": "permission_denied" })).is_ok());
        assert!(sanitize_for_action(action, &json!({ "reason": "free text" })).is_err());
        assert!(sanitize_for_action(action, &json!({ "canary": "x" })).is_err());
        assert!(sanitize_for_action(action, &json!({ "nested": {"a": 1} })).is_err());
    }

    #[test]
    fn allowlist_rejects_name_slug_filename_content_type_free_strings() {
        assert!(sanitize_for_action(
            AuditAction::CollectionCreate,
            &json!({ "name": "CANARY free name", "collection_id": "11111111-1111-1111-1111-111111111111" })
        )
        .is_err());
        assert!(sanitize_for_action(
            AuditAction::CollectionCreate,
            &json!({ "slug": "canary-slug", "collection_id": "11111111-1111-1111-1111-111111111111" })
        )
        .is_err());
        assert!(sanitize_for_action(
            AuditAction::DocumentUpload,
            &json!({ "filename": "secret.pdf", "reason": "upload_accepted" })
        )
        .is_err());
        assert!(sanitize_for_action(
            AuditAction::DocumentUpload,
            &json!({ "content_type": "text/plain", "reason": "upload_accepted" })
        )
        .is_err());
        // Counts/IDs only — 200-char collection names never enter audit metadata.
        assert!(sanitize_for_action(
            AuditAction::CollectionCreate,
            &json!({
                "collection_id": "11111111-1111-1111-1111-111111111111",
                "name_chars": 200,
                "slug_chars": 12
            })
        )
        .is_ok());
    }

    #[test]
    fn route_matrix_ask_search_upload_reindex_have_stable_deny_codes() {
        for (action, resource) in [
            (AuditAction::DocumentAsk, AuditResource::Ask),
            (AuditAction::DocumentSearch, AuditResource::Search),
            (AuditAction::DocumentUpload, AuditResource::Document),
            (AuditAction::DocumentReindex, AuditResource::Document),
        ] {
            assert!(sanitize_for_action(
                action,
                &json!({ "reason": AuditReason::PermissionDenied.as_str() })
            )
            .is_ok());
            assert_eq!(
                AuditReason::parse("permission_denied").unwrap(),
                AuditReason::PermissionDenied
            );
            let _ = (action.as_str(), resource.as_str());
        }
        assert_eq!(AuditAction::DocumentAsk.as_str(), "ask.query");
        assert_eq!(AuditAction::DocumentSearch.as_str(), "search.query");
        assert_eq!(AuditAction::DocumentUpload.as_str(), "document.upload");
        assert_eq!(AuditAction::DocumentReindex.as_str(), "document.reindex");
    }

    #[test]
    fn central_typed_route_audit_covers_success_deny_error_matrix() {
        let routes = [
            RouteAuditKind::Upload,
            RouteAuditKind::Ask,
            RouteAuditKind::AskStream,
            RouteAuditKind::Search,
            RouteAuditKind::CollectionCreate,
            RouteAuditKind::CollectionUpdate,
            RouteAuditKind::CollectionDelete,
            RouteAuditKind::DocumentGet,
            RouteAuditKind::DocumentDelete,
            RouteAuditKind::DocumentReindex,
        ];
        for kind in routes {
            for outcome in [
                AuditOutcome::Success,
                AuditOutcome::Deny,
                AuditOutcome::Error,
            ] {
                let meta = match outcome {
                    AuditOutcome::Success => json!({}),
                    _ => json!({ "reason": AuditReason::PermissionDenied.as_str() }),
                };
                assert!(
                    sanitize_for_action(kind.action(), &meta).is_ok(),
                    "{kind:?}/{outcome:?}"
                );
                assert!(!kind.action().as_str().is_empty());
                assert!(!kind.resource().as_str().is_empty());
            }
        }
    }

    #[test]
    fn deny_and_success_records_preserve_request_id_for_correlation() {
        let request_id = "11111111-1111-1111-1111-111111111111";
        let deny = AuditRecord {
            request_id,
            action: "document.delete",
            resource_type: "document",
            resource_id: Some("doc-1"),
            outcome: AuditOutcome::Deny,
            metadata: json!({ "reason": "permission_denied" }),
        };
        let success = AuditRecord {
            request_id,
            action: "document.delete",
            resource_type: "document",
            resource_id: Some("doc-1"),
            outcome: AuditOutcome::Success,
            metadata: json!({}),
        };
        assert_eq!(deny.request_id, success.request_id);
        assert_eq!(deny.outcome, AuditOutcome::Deny);
        let envelope = to_telemetry_envelope(TelemetryAuditInput {
            org_id: Uuid::nil(),
            actor_id: Uuid::nil(),
            request_id,
            action: "document.delete",
            resource_type: "document",
            resource_id: "doc-1",
            outcome: AuditOutcome::Deny,
            metadata: &[],
        });
        assert_eq!(envelope.request_id, request_id);
        assert_eq!(envelope.outcome, "deny");
    }
}
