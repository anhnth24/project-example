//! Shared route helpers for `/api/v1` collection/document/job contracts.

use axum::http::HeaderMap;
use uuid::Uuid;

use crate::api::ApiRejection;
use crate::auth::context::OrgContext;
use crate::auth::permissions::{require_collection, require_permission, ResolveError};
use crate::db::error::DbError;
use crate::db::models::{Collection, Document, DocumentVersion, Job};
use crate::db::pool::with_org_txn;
use crate::http::AppState;

pub fn map_resolve(error: ResolveError, request_id: &str) -> ApiRejection {
    match error {
        ResolveError::PermissionDenied => ApiRejection::permission_denied(request_id),
        ResolveError::CollectionDenied => ApiRejection::collection_denied(request_id),
        ResolveError::UserDisabled => ApiRejection::new(
            axum::http::StatusCode::FORBIDDEN,
            "user_disabled",
            "User account is disabled",
            request_id,
        ),
        ResolveError::MembershipMissing => ApiRejection::new(
            axum::http::StatusCode::FORBIDDEN,
            "membership_missing",
            "Org membership is missing",
            request_id,
        ),
        ResolveError::InvalidContext | ResolveError::Database => ApiRejection::internal(request_id),
    }
}

pub fn require_perm(ctx: &OrgContext, code: &str, request_id: &str) -> Result<(), ApiRejection> {
    require_permission(ctx, code).map_err(|error| map_resolve(error, request_id))
}

pub fn require_coll(
    ctx: &OrgContext,
    collection_id: Uuid,
    request_id: &str,
) -> Result<(), ApiRejection> {
    require_collection(ctx, collection_id).map_err(|error| map_resolve(error, request_id))
}

/// IDOR-safe deny: missing and unauthorized collection scope both look like 404.
pub fn deny_or_not_found(request_id: &str) -> ApiRejection {
    ApiRejection::not_found("Resource not found", request_id)
}

pub fn map_db(error: DbError, request_id: &str) -> ApiRejection {
    match error {
        DbError::NotFound => deny_or_not_found(request_id),
        DbError::StaleState { .. } => {
            ApiRejection::conflict("conflict_state", "Resource state conflict", request_id)
        }
        DbError::IllegalTransition { .. } => {
            ApiRejection::conflict("illegal_transition", "Illegal state transition", request_id)
        }
        DbError::Config(ref message) if message == "invalid_resolution_version" => {
            ApiRejection::validation(
                "Resolution version IDs must be published versions on each conflict side",
                request_id,
            )
        }
        DbError::Config(ref message) if message == "idempotency_key_conflict" => {
            ApiRejection::conflict(
                "idempotency_key_conflict",
                "Idempotency-Key was reused with a different request",
                request_id,
            )
        }
        DbError::Config(ref message) if message == "idempotency_in_progress" => {
            ApiRejection::conflict(
                "idempotency_in_progress",
                "Idempotency-Key request is still in progress",
                request_id,
            )
        }
        DbError::Config(ref message) if message == "idempotency_finalize_failed" => {
            ApiRejection::internal(request_id)
        }
        DbError::Config(ref message) if message == "version_superseded" => ApiRejection::conflict(
            "version_superseded",
            "Version is already published and not current",
            request_id,
        ),
        DbError::Config(ref message) if message == "invalid_publish" => {
            ApiRejection::validation("Publish request is invalid", request_id)
        }
        DbError::Config(message)
            if message.contains("unique")
                || message.contains("duplicate")
                || message.contains("violates") =>
        {
            ApiRejection::conflict("conflict", "Resource conflict", request_id)
        }
        _ => ApiRejection::internal(request_id),
    }
}

pub fn validate_idempotency_key(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 128 {
        return Err("Idempotency-Key must be between 1 and 128 bytes".into());
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err("Idempotency-Key contains unsupported characters".into());
    }
    Ok(())
}

pub fn read_idempotency_key(
    headers: &HeaderMap,
    request_id: &str,
) -> Result<Option<String>, ApiRejection> {
    let Some(value) = headers.get("idempotency-key") else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| {
        ApiRejection::validation("Idempotency-Key must be visible ASCII", request_id)
    })?;
    validate_idempotency_key(value)
        .map_err(|message| ApiRejection::validation(message, request_id))?;
    Ok(Some(value.to_string()))
}

pub fn parse_slug(value: &str) -> Result<&str, String> {
    if value.len() < 2 || value.len() > 63 {
        return Err("slug must be 2..=63 characters".into());
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err("slug must be 2..=63 characters".into());
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err("slug must match ^[a-z0-9][a-z0-9-]{1,62}$".into());
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-') {
        return Err("slug must match ^[a-z0-9][a-z0-9-]{1,62}$".into());
    }
    Ok(value)
}

pub fn collection_response(
    collection: Collection,
    request_id: String,
) -> crate::api::CollectionResponse {
    crate::api::CollectionResponse {
        id: collection.id,
        name: collection.name,
        slug: collection.slug,
        description: collection.description,
        visibility: collection.visibility.as_str().into(),
        owner_user_id: collection.owner_user_id,
        created_at: collection.created_at,
        updated_at: collection.updated_at,
        request_id,
    }
}

pub fn document_response(document: Document, request_id: String) -> crate::api::DocumentResponse {
    crate::api::DocumentResponse {
        id: document.id,
        collection_id: document.collection_id,
        title: document.title,
        state: document.state.as_str().into(),
        current_version_id: document.current_version_id,
        created_by_user_id: document.created_by_user_id,
        created_at: document.created_at,
        updated_at: document.updated_at,
        deleted_at: document.deleted_at,
        request_id,
    }
}

pub fn version_response(
    version: DocumentVersion,
    request_id: String,
) -> crate::api::DocumentVersionResponse {
    crate::api::DocumentVersionResponse {
        id: version.id,
        document_id: version.document_id,
        version_number: version.version_number,
        parent_version_id: version.parent_version_id,
        publication_state: match version.publication_state {
            crate::db::models::PublicationState::Draft => "draft".into(),
            crate::db::models::PublicationState::Published => "published".into(),
        },
        is_current: version.is_current,
        content_sha256: version.content_sha256,
        // Never expose object keys / bucket internals on the wire.
        source_filename: version.source_filename,
        source_content_type: version.source_content_type,
        byte_size: version.byte_size,
        effective_from: version.effective_from,
        effective_to: version.effective_to,
        change_summary: version.change_summary,
        created_by_user_id: version.created_by_user_id,
        created_at: version.created_at,
        request_id,
    }
}

pub fn job_response(job: Job, request_id: String) -> crate::api::JobResponse {
    crate::api::JobResponse {
        id: job.id,
        job_type: job.job_type.as_str().into(),
        status: job.status.as_str().into(),
        attempts: job.attempts,
        max_attempts: job.max_attempts,
        document_id: job.document_id,
        version_id: job.version_id,
        available_at: job.available_at,
        started_at: job.started_at,
        finished_at: job.finished_at,
        // last_error is already sanitized at the job boundary.
        last_error: job.last_error,
        created_at: job.created_at,
        updated_at: job.updated_at,
        request_id,
    }
}

pub async fn load_document_authorized(
    state: &AppState,
    ctx: &OrgContext,
    document_id: Uuid,
    request_id: &str,
) -> Result<Document, ApiRejection> {
    let document = with_org_txn(state.pool(), ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move { crate::db::documents::get_by_id(txn, &ctx, document_id).await })
        }
    })
    .await
    .map_err(|error| map_db(error, request_id))?;
    if !ctx.allows_collection(document.collection_id) {
        return Err(deny_or_not_found(request_id));
    }
    Ok(document)
}
