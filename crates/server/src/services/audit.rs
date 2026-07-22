//! Safe append-only audit service (typed actions, fail-closed mutations, durable denies).

use serde_json::{json, Value as JsonValue};
use tokio_postgres::Transaction;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::audit::{self, NewAuditEvent};
use crate::db::error::DbError;
use crate::telemetry::CorrelationContext;

/// Stable audit action names used across routes/services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    AuthLogin,
    AuthDeny,
    AuthLogout,
    AuthRefresh,
    AuthRefreshReuse,
    AuthRevokeAll,
    DocumentUpload,
    DocumentDelete,
    DocumentTombstone,
    DocumentPublish,
    DocumentReindex,
    DocumentPurge,
    DocumentPurgeObjects,
    JobEnqueue,
    QuotaDeny,
    ReconcileRepair,
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
            Self::DocumentUpload => "document.upload",
            Self::DocumentDelete => "document.delete",
            Self::DocumentTombstone => "document.tombstone",
            Self::DocumentPublish => "document.publish",
            Self::DocumentReindex => "document.reindex",
            Self::DocumentPurge => "document.purge",
            Self::DocumentPurgeObjects => "document.purge_objects",
            Self::JobEnqueue => "job.enqueue",
            Self::QuotaDeny => "quota.deny",
            Self::ReconcileRepair => "reconcile.repair",
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
            "document.upload" => Ok(Self::DocumentUpload),
            "document.delete" => Ok(Self::DocumentDelete),
            "document.tombstone" => Ok(Self::DocumentTombstone),
            "document.publish" => Ok(Self::DocumentPublish),
            "document.reindex" => Ok(Self::DocumentReindex),
            "document.purge" => Ok(Self::DocumentPurge),
            "document.purge_objects" => Ok(Self::DocumentPurgeObjects),
            "job.enqueue" => Ok(Self::JobEnqueue),
            "quota.deny" => Ok(Self::QuotaDeny),
            "reconcile.repair" => Ok(Self::ReconcileRepair),
            "vector.cleanup_intent" => Ok(Self::VectorCleanupIntent),
            "object.cleanup" => Ok(Self::ObjectCleanup),
            _ => Err(format!("audit_action_invalid:{value}")),
        }
    }

    /// Exact scalar metadata keys permitted for this action.
    pub fn metadata_keys(self) -> &'static [&'static str] {
        match self {
            // Login/logout emit opaque family/refresh ids; deny stays reason/error only.
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
            Self::DocumentUpload => &["reason", "format", "document_id", "version_id"],
            Self::DocumentDelete | Self::DocumentTombstone => &[
                "reason",
                "document_id",
                "version_id",
                "cancelled_writer_jobs",
            ],
            Self::DocumentPublish | Self::DocumentReindex => {
                &["document_id", "version_id", "job_id", "job_type"]
            }
            Self::DocumentPurge | Self::DocumentPurgeObjects => &[
                "document_id",
                "phase",
                "object_count",
                "deleted_chunks",
                "cancelled_writer_jobs",
                "job_id",
            ],
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
            Self::VectorCleanupIntent => &["document_id", "phase", "result"],
            Self::ObjectCleanup => &["document_id", "phase", "object_count"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResource {
    Session,
    Document,
    Job,
    Quota,
    Object,
}

impl AuditResource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Document => "document",
            Self::Job => "job",
            Self::Quota => "quota",
            Self::Object => "object",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "session" => Ok(Self::Session),
            "document" => Ok(Self::Document),
            "job" => Ok(Self::Job),
            "quota" => Ok(Self::Quota),
            "object" => Ok(Self::Object),
            _ => Err(format!("audit_resource_type_invalid:{value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    Success,
    Deny,
    Error,
}

impl AuditOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Deny => "deny",
            Self::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "success" => Ok(Self::Success),
            "deny" => Ok(Self::Deny),
            "error" => Ok(Self::Error),
            _ => Err(format!("audit_outcome_invalid:{value}")),
        }
    }
}

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
    /// Refresh token past `expires_at` (auth.refresh deny).
    Expired,
    /// Lost rotation race under family lock (auth.refresh.reuse deny).
    RefreshRace,
    UserRequested,
    UploadAccepted,
    QuotaExceeded,
    System,
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
            Self::System => "system",
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
            "system" => Ok(Self::System),
            _ => Err(format!("audit_reason_invalid:{value}")),
        }
    }
}

/// Compatibility string constants for existing call sites.
pub mod actions {
    pub const AUTH_LOGIN: &str = "auth.login";
    pub const AUTH_DENY: &str = "auth.deny";
    pub const AUTH_LOGOUT: &str = "auth.logout";
    pub const AUTH_REFRESH: &str = "auth.refresh";
    pub const AUTH_REFRESH_REUSE: &str = "auth.refresh.reuse";
    pub const AUTH_REVOKE_ALL: &str = "auth.revoke_all";
    pub const DOCUMENT_UPLOAD: &str = "document.upload";
    pub const DOCUMENT_DELETE: &str = "document.delete";
    pub const DOCUMENT_TOMBSTONE: &str = "document.tombstone";
    pub const DOCUMENT_PUBLISH: &str = "document.publish";
    pub const DOCUMENT_REINDEX: &str = "document.reindex";
    pub const DOCUMENT_PURGE: &str = "document.purge";
    pub const DOCUMENT_PURGE_OBJECTS: &str = "document.purge_objects";
    pub const JOB_ENQUEUE: &str = "job.enqueue";
    pub const QUOTA_DENY: &str = "quota.deny";
    pub const RECONCILE_REPAIR: &str = "reconcile.repair";
    pub const VECTOR_CLEANUP_INTENT: &str = "vector.cleanup_intent";
    pub const OBJECT_CLEANUP: &str = "object.cleanup";
}

pub mod resources {
    pub const SESSION: &str = "session";
    pub const DOCUMENT: &str = "document";
    pub const JOB: &str = "job";
    pub const QUOTA: &str = "quota";
    pub const OBJECT: &str = "object";
}

/// Append-only audit fields (metadata must never contain secrets).
pub struct AuditEvent<'a> {
    pub org_id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub action: &'a str,
    pub resource_type: &'a str,
    pub resource_id: Option<&'a str>,
    pub outcome: &'a str,
    pub request_id: &'a str,
    pub metadata: JsonValue,
}

fn sanitize_for_action(action: AuditAction, metadata: &JsonValue) -> Result<JsonValue, String> {
    let JsonValue::Object(map) = metadata else {
        return Err("audit_metadata_must_be_object".into());
    };
    let allowed = action.metadata_keys();
    let mut filtered = serde_json::Map::new();
    for (key, value) in map {
        if !allowed.iter().any(|item| *item == key) {
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
                filtered.insert(key.clone(), JsonValue::String(text.clone()));
            }
            _ => return Err("audit_metadata_value_must_be_scalar".into()),
        }
    }
    crate::telemetry::sanitize_audit_metadata(&JsonValue::Object(filtered))
}

/// Append-only audit row with allowlisted metadata.
pub async fn write_audit(txn: &Transaction<'_>, event: AuditEvent<'_>) -> Result<(), DbError> {
    let action = AuditAction::parse(event.action).map_err(DbError::Config)?;
    let resource = AuditResource::parse(event.resource_type).map_err(DbError::Config)?;
    let outcome = AuditOutcome::parse(event.outcome).map_err(DbError::Config)?;
    let metadata = sanitize_for_action(action, &event.metadata).map_err(DbError::Config)?;
    audit::append_raw(
        txn,
        event.org_id,
        NewAuditEvent {
            actor_user_id: event.actor_user_id,
            action: action.as_str(),
            resource_type: resource.as_str(),
            resource_id: event.resource_id,
            outcome: outcome.as_str(),
            request_id: event.request_id,
            metadata,
        },
    )
    .await
    .map(|_| ())
}

pub async fn write_for_org(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    event: NewAuditEvent<'_>,
) -> Result<(), DbError> {
    let action = AuditAction::parse(event.action).map_err(DbError::Config)?;
    let resource = AuditResource::parse(event.resource_type).map_err(DbError::Config)?;
    let outcome = AuditOutcome::parse(event.outcome).map_err(DbError::Config)?;
    let metadata = sanitize_for_action(action, &event.metadata).map_err(DbError::Config)?;
    audit::append(
        txn,
        ctx,
        NewAuditEvent {
            actor_user_id: event.actor_user_id,
            action: action.as_str(),
            resource_type: resource.as_str(),
            resource_id: event.resource_id,
            outcome: outcome.as_str(),
            request_id: event.request_id,
            metadata,
        },
    )
    .await
    .map(|_| ())
}

/// Convenience builder for org-scoped audit writes.
#[allow(clippy::too_many_arguments)]
pub async fn write_org_action(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    action: &str,
    resource_type: &str,
    resource_id: Option<&str>,
    outcome: &str,
    request_id: &str,
    metadata: JsonValue,
) -> Result<(), DbError> {
    write_for_org(
        txn,
        ctx,
        NewAuditEvent {
            actor_user_id: Some(ctx.user_id()),
            action,
            resource_type,
            resource_id,
            outcome,
            request_id,
            metadata,
        },
    )
    .await
}

pub fn request_id_from_correlation() -> String {
    CorrelationContext::current()
        .and_then(|ctx| {
            Uuid::parse_str(&ctx.request_id)
                .ok()
                .map(|id| id.to_string())
        })
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

pub fn reason_metadata(reason: AuditReason) -> JsonValue {
    json!({ "reason": reason.as_str() })
}

/// Durable deny policy: never silently ignore audit failures.
///
/// Tries a transactional insert; on failure appends a fallback structured event and
/// still returns `Ok(())` so callers can return a safe deny response. Mutation paths
/// must use [`write_audit`] / [`write_org_action`] with `?` instead (fail-closed).
#[allow(clippy::too_many_arguments)]
pub async fn write_deny_durable(
    pool: &deadpool_postgres::Pool,
    org_id: Uuid,
    actor_user_id: Option<Uuid>,
    action: AuditAction,
    resource: AuditResource,
    resource_id: Option<&str>,
    request_id: &str,
    metadata: JsonValue,
) -> Result<(), DbError> {
    let request_id = if Uuid::parse_str(request_id).is_ok() {
        request_id.to_string()
    } else {
        Uuid::new_v4().to_string()
    };
    let resource_id_owned = resource_id.map(str::to_string);
    let write = crate::db::pool::with_org_txn_typed(
        pool,
        &OrgContext::try_new(org_id, actor_user_id.unwrap_or(org_id), ["doc.upload"], [])
            .map_err(|error| DbError::Config(error.to_string()))?,
        {
            let request_id = request_id.clone();
            let metadata = metadata.clone();
            let resource_id_owned = resource_id_owned.clone();
            move |txn| {
                Box::pin(async move {
                    write_audit(
                        txn,
                        AuditEvent {
                            org_id,
                            actor_user_id,
                            action: action.as_str(),
                            resource_type: resource.as_str(),
                            resource_id: resource_id_owned.as_deref(),
                            outcome: AuditOutcome::Deny.as_str(),
                            request_id: &request_id,
                            metadata,
                        },
                    )
                    .await
                })
            }
        },
    )
    .await;

    if let Err(error) = write {
        tracing::error!(
            target: "audit",
            request_id = %request_id,
            action = action.as_str(),
            outcome = "deny",
            error_class = "audit_write_failed",
            "deny audit write failed; emitting fallback append"
        );
        // Fallback append mechanism: durable-enough process log with allowlisted fields only.
        tracing::warn!(
            target: "audit_fallback",
            request_id = %request_id,
            org_id = %org_id,
            action = action.as_str(),
            resource_type = resource.as_str(),
            outcome = "deny",
            fallback = true,
            "audit_fallback_deny"
        );
        let _ = error;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_names_are_stable_and_non_empty() {
        for action in [
            AuditAction::DocumentUpload,
            AuditAction::DocumentDelete,
            AuditAction::DocumentPublish,
            AuditAction::DocumentReindex,
            AuditAction::AuthDeny,
            AuditAction::QuotaDeny,
        ] {
            assert!(!action.as_str().is_empty());
            assert!(action.as_str().contains('.'));
            assert!(AuditAction::parse(action.as_str()).is_ok());
        }
    }

    #[test]
    fn rejects_nested_metadata_and_unknown_keys() {
        let err = sanitize_for_action(
            AuditAction::DocumentUpload,
            &json!({"reason": "upload_accepted", "nested": {"a": 1}}),
        )
        .unwrap_err();
        assert!(err.contains("scalar") || err.contains("allowlisted"));
        assert!(sanitize_for_action(
            AuditAction::QuotaDeny,
            &json!({"reason": "quota_exceeded", "resource_kind": "documents"}),
        )
        .is_ok());
    }

    #[test]
    fn auth_and_purge_allowlists_match_emitted_ids_and_reasons() {
        assert!(sanitize_for_action(
            AuditAction::AuthLogin,
            &json!({
                "family_id": "550e8400-e29b-41d4-a716-446655440000",
                "refresh_id": "550e8400-e29b-41d4-a716-446655440001"
            }),
        )
        .is_ok());
        assert!(sanitize_for_action(
            AuditAction::AuthLogout,
            &json!({ "family_id": "550e8400-e29b-41d4-a716-446655440000" }),
        )
        .is_ok());
        assert!(
            sanitize_for_action(AuditAction::AuthRefresh, &json!({ "reason": "expired" }),).is_ok()
        );
        assert!(sanitize_for_action(
            AuditAction::AuthRefreshReuse,
            &json!({ "reason": "refresh_race" }),
        )
        .is_ok());
        assert!(sanitize_for_action(
            AuditAction::DocumentPurge,
            &json!({
                "document_id": "550e8400-e29b-41d4-a716-446655440000",
                "job_id": "550e8400-e29b-41d4-a716-446655440002",
                "deleted_chunks": 3
            }),
        )
        .is_ok());
        // Secrets / unknown keys stay rejected.
        assert!(sanitize_for_action(AuditAction::AuthLogin, &json!({ "password": "x" }),).is_err());
        assert!(sanitize_for_action(
            AuditAction::AuthDeny,
            &json!({ "family_id": "550e8400-e29b-41d4-a716-446655440000" }),
        )
        .is_err());
    }
}
