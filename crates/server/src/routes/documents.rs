//! Document/version/preview/download/citation/conflict routes (P1B-R02/R04).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::{
    decode_cursor, encode_cursor, ApiError, DocumentDto, DocumentVersionDto, Page, PageInfo,
    Pagination,
};
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::error::DbError;
use crate::db::models::AuditOutcome;
use crate::db::models::JobType;
use crate::db::pool::{with_org_txn, with_org_txn_typed};
use crate::db::{document_versions, documents};
use crate::http::AppState;
use crate::jobs::{self, EnqueueJob, JobPayload};
use crate::services::access::{self, AccessError};
use crate::services::audit;
use crate::services::citation::{resolve_citation, ResolveCitationRequest};
use crate::services::deletion;
use crate::services::download::{self, DownloadPurpose};
use crate::services::preview;
use crate::services::retrieval::PERMISSION_QA_HISTORY;
use crate::services::upload::{approve_quarantined_upload, ApproveIntakeRequest, SagaError};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/collections/{collection_id}/documents",
            get(list_documents),
        )
        .route(
            "/api/v1/documents/{document_id}",
            get(get_document).delete(delete_document),
        )
        .route(
            "/api/v1/collections/{collection_id}/documents/{document_id}/approve-intake",
            post(approve_intake),
        )
        .route(
            "/api/v1/documents/{document_id}/preview",
            get(preview_document),
        )
        .route(
            "/api/v1/documents/{document_id}/versions",
            get(list_versions),
        )
        .route(
            "/api/v1/documents/{document_id}/versions/{version_id}",
            get(get_version),
        )
        .route(
            "/api/v1/documents/{document_id}/versions/{version_id}/publish",
            post(publish_version),
        )
        .route(
            "/api/v1/documents/{document_id}/versions/{version_id}/download-capability",
            post(issue_download),
        )
        .route("/api/v1/downloads/{capability}", get(redeem_download))
        .route(
            "/api/v1/documents/{document_id}/reindex",
            post(reindex_document),
        )
        .route("/api/v1/citations/resolve", post(resolve_citation_route))
        .route("/api/v1/conflicts", get(list_conflicts))
        .route("/api/v1/conflicts/{conflict_id}", get(get_conflict))
        .route(
            "/api/v1/conflicts/{conflict_id}/evidence",
            get(get_conflict_evidence),
        )
        .route(
            "/api/v1/conflicts/{conflict_id}/triage",
            post(triage_conflict),
        )
        .route(
            "/api/v1/documents/{document_id}/versions/{version_id}/diff",
            get(version_diff),
        )
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreviewQuery {
    version_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct VersionDiffQuery {
    against: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TriageConflictRequest {
    status: String,
    resolution_note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadCapabilityRequest {
    purpose: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveBody {
    logical_document_id: Uuid,
    version_id: Uuid,
    source_content_sha256: String,
    canonical_markdown_sha256: String,
    chunk_id: Uuid,
    source_span_start: usize,
    source_span_end: usize,
    quote_local_start: usize,
    quote_local_end: usize,
    quote: String,
    #[serde(default)]
    require_current: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApproveIntakeBody {
    reason: Option<String>,
}

async fn approve_intake(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path((collection_id, document_id)): Path<(Uuid, Uuid)>,
    body: Option<Json<ApproveIntakeBody>>,
) -> Result<Json<serde_json::Value>, RouteError> {
    let reason = body.and_then(|Json(b)| b.reason);
    let registered = approve_quarantined_upload(
        state.pool(),
        &auth.context,
        ApproveIntakeRequest {
            collection_id,
            document_id,
            reason: reason.as_deref(),
            request_id: &auth.request_id,
        },
    )
    .await
    .map_err(|error| match error {
        SagaError::PermissionDenied => RouteError::Denied(auth.request_id.clone()),
        SagaError::NotFound => RouteError::NotFound(auth.request_id.clone()),
        SagaError::Database(_) | SagaError::Job(_) | SagaError::Internal => {
            RouteError::Database(auth.request_id.clone())
        }
        _ => RouteError::Validation(
            auth.request_id.clone(),
            "Upload is not awaiting intake approval",
        ),
    })?;
    let job_id = registered
        .job_id
        .ok_or_else(|| RouteError::Database(auth.request_id.clone()))?;
    Ok(Json(serde_json::json!({
        "documentId": registered.document_id,
        "versionId": registered.version_id,
        "collectionId": registered.collection_id,
        "jobId": job_id,
        "created": registered.created_job,
        "requestId": auth.request_id,
    })))
}

async fn list_documents(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(collection_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<DocumentDto>>, RouteError> {
    if !auth.context.allows_collection(collection_id) {
        return Err(RouteError::Denied(auth.request_id));
    }
    let pagination = Pagination::from_query(query.limit);
    let (after_at, after_id) = match query.cursor.as_deref() {
        Some(raw) => decode_cursor(raw)
            .map(|(at, id)| (Some(at), Some(id)))
            .ok_or_else(|| RouteError::Validation(auth.request_id.clone(), "Invalid cursor"))?,
        None => (None, None),
    };
    let mut rows = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                documents::list_in_collection(
                    txn,
                    &ctx,
                    collection_id,
                    pagination.limit + 1,
                    after_at,
                    after_id,
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    let has_more = rows.len() as i64 > pagination.limit;
    if has_more {
        rows.truncate(pagination.limit as usize);
    }
    let next_cursor = rows.last().map(|row| encode_cursor(row.created_at, row.id));
    Ok(Json(Page {
        items: rows.into_iter().map(document_dto).collect(),
        page: PageInfo {
            next_cursor,
            has_more,
        },
    }))
}

async fn get_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(document_id): Path<Uuid>,
) -> Result<Json<DocumentDto>, RouteError> {
    let row = load_document(&state, &auth, document_id).await?;
    Ok(Json(document_dto(row)))
}

async fn delete_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(document_id): Path<Uuid>,
) -> Result<StatusCode, RouteError> {
    if require_permission(&auth.context, "doc.delete").is_err() {
        let resource_id = document_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "document.delete",
            "document",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let _ = load_document(&state, &auth, document_id).await?;
    deletion::request_delete(state.pool(), &auth.context, document_id)
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    let resource_id = document_id.to_string();
    audit::record(
        state.pool(),
        &auth.context,
        audit::AuditRecord {
            request_id: &auth.request_id,
            action: "document.delete",
            resource_type: "document",
            resource_id: Some(&resource_id),
            outcome: AuditOutcome::Success,
            metadata: serde_json::json!({}),
        },
    )
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn preview_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(document_id): Path<Uuid>,
    Query(query): Query<PreviewQuery>,
) -> Result<Json<serde_json::Value>, RouteError> {
    let store = state
        .object_store()
        .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
    let preview = preview::preview_markdown(
        state.pool(),
        &auth.context,
        store,
        document_id,
        query.version_id,
    )
    .await
    .map_err(|error| RouteError::from_preview(error, &auth.request_id))?;
    let resource_id = document_id.to_string();
    audit::record(
        state.pool(),
        &auth.context,
        audit::AuditRecord {
            request_id: &auth.request_id,
            action: "document.preview",
            resource_type: "document",
            resource_id: Some(&resource_id),
            outcome: AuditOutcome::Success,
            metadata: serde_json::json!({
                "document_id": document_id.to_string(),
                "version_id": preview.version.id.to_string(),
            }),
        },
    )
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    Ok(Json(serde_json::json!({
        "documentId": preview.document.id,
        "versionId": preview.version.id,
        "versionNumber": preview.version.version_number,
        "sourceContentSha256": preview.source_content_sha256,
        "canonicalMarkdownSha256": preview.canonical_markdown_sha256,
        "isCurrent": preview.version.is_current,
        "truncated": preview.truncated,
        "markdown": preview.markdown,
        "requestId": auth.request_id,
    })))
}

async fn list_versions(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(document_id): Path<Uuid>,
) -> Result<Json<Page<DocumentVersionDto>>, RouteError> {
    let _ = load_document(&state, &auth, document_id).await?;
    require_permission(&auth.context, "qa.query")
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let rows = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                document_versions::list_published_for_document(txn, &ctx, document_id, 100).await
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    let has_history = auth.context.has_permission(PERMISSION_QA_HISTORY);
    let items = rows
        .into_iter()
        .filter(|row| row.is_current || has_history)
        .map(version_dto)
        .collect();
    Ok(Json(Page {
        items,
        page: PageInfo {
            next_cursor: None,
            has_more: false,
        },
    }))
}

async fn get_version(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path((document_id, version_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<DocumentVersionDto>, RouteError> {
    let authorized = access::resolve_published_version(
        state.pool(),
        &auth.context,
        document_id,
        Some(version_id),
    )
    .await
    .map_err(|error| RouteError::from_access(error, &auth.request_id))?;
    Ok(Json(version_dto(authorized.version)))
}

async fn version_diff(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path((document_id, version_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<VersionDiffQuery>,
) -> Result<Json<serde_json::Value>, RouteError> {
    require_permission(&auth.context, PERMISSION_QA_HISTORY)
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let left = access::resolve_published_version(
        state.pool(),
        &auth.context,
        document_id,
        Some(version_id),
    )
    .await
    .map_err(|error| RouteError::from_access(error, &auth.request_id))?;
    let right_id = query.against.ok_or_else(|| {
        RouteError::Validation(auth.request_id.clone(), "against version id required")
    })?;
    let right =
        access::resolve_published_version(state.pool(), &auth.context, document_id, Some(right_id))
            .await
            .map_err(|error| RouteError::from_access(error, &auth.request_id))?;
    Ok(Json(serde_json::json!({
        "documentId": document_id,
        "left": version_dto(left.version),
        "right": version_dto(right.version),
        "note": "Structured compare retrieval remains available via /search mode=compare",
        "requestId": auth.request_id,
    })))
}

async fn publish_version(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path((document_id, version_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, RouteError> {
    require_permission(&auth.context, "doc.publish")
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let _ = load_document(&state, &auth, document_id).await?;
    with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                document_versions::publish_version(txn, &ctx, document_id, version_id).await
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn issue_download(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path((document_id, version_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<DownloadCapabilityRequest>,
) -> Result<Json<serde_json::Value>, RouteError> {
    let keys = state
        .capability_keys()
        .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
    let purpose = match body.purpose.as_str() {
        "markdown" => DownloadPurpose::Markdown,
        "original" => DownloadPurpose::Original,
        _ => {
            return Err(RouteError::Validation(
                auth.request_id.clone(),
                "purpose must be markdown or original",
            ))
        }
    };
    let issued = download::issue_capability(
        state.pool(),
        &auth.context,
        keys,
        document_id,
        version_id,
        purpose,
        None,
    )
    .await
    .map_err(|error| RouteError::from_download(error, &auth.request_id))?;
    Ok(Json(serde_json::json!({
        "capability": issued.token.expose(),
        "expiresIn": issued.expires_in,
        "purpose": purpose.as_str(),
        "documentId": issued.document_id,
        "versionId": issued.version_id,
        "requestId": auth.request_id,
    })))
}

async fn redeem_download(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(capability): Path<String>,
) -> Result<Response, RouteError> {
    let keys = state
        .capability_keys()
        .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
    let store = state
        .object_store()
        .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
    let bytes = download::redeem_capability(state.pool(), &auth.context, keys, store, &capability)
        .await
        .map_err(|error| RouteError::from_download(error, &auth.request_id))?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, bytes.content_type.parse().unwrap());
    headers.insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_static("attachment"),
    );
    Ok((StatusCode::OK, headers, bytes.bytes).into_response())
}

async fn reindex_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    client_ip: Option<axum::Extension<crate::middleware::ClientIp>>,
    Path(document_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RouteError> {
    let ip = client_ip
        .map(|ext| ext.0 .0.clone())
        .unwrap_or_else(|| "unknown".into());
    crate::routes::rate_limit_guard::check_user(
        &state,
        &auth.context.org_id().to_string(),
        &auth.context.user_id().to_string(),
        &auth.request_id,
    )
    .map_err(RouteError::RateLimited)?;
    crate::routes::rate_limit_guard::check_route(&state, "reindex", &ip, &auth.request_id)
        .map_err(RouteError::RateLimited)?;
    if require_permission(&auth.context, "doc.upload").is_err() {
        let resource_id = document_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "document.reindex",
            "document",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let document = load_document(&state, &auth, document_id).await?;
    let version_id = document.current_version_id.ok_or_else(|| {
        RouteError::Validation(auth.request_id.clone(), "Document has no current version")
    })?;
    let idem = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| format!("reindex:{document_id}:{version_id}"));
    if idem.len() > 160 {
        let resource_id = document_id.to_string();
        audit::record(
            state.pool(),
            &auth.context,
            audit::AuditRecord {
                request_id: &auth.request_id,
                action: "document.reindex",
                resource_type: "document",
                resource_id: Some(&resource_id),
                outcome: AuditOutcome::Error,
                metadata: serde_json::json!({
                    "reason": "validation_failed",
                    "document_id": document_id.to_string(),
                    "version_id": version_id.to_string(),
                }),
            },
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Idempotency key too long",
        ));
    }
    let payload = JobPayload {
        document_id: Some(document_id),
        version_id: Some(version_id),
        ..JobPayload::default()
    };
    // Mutations/enqueue + success audit must commit in the same transaction.
    let outcome = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = auth.request_id.clone();
        move |txn| {
            Box::pin(async move {
                let outcome = jobs::enqueue_within_txn(
                    txn,
                    &ctx,
                    EnqueueJob::new(JobType::Index, payload, idem),
                )
                .await?;
                let resource_id = document_id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    audit::AuditRecord {
                        request_id: &request_id,
                        action: "document.reindex",
                        resource_type: "document",
                        resource_id: Some(&resource_id),
                        outcome: AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "document_id": document_id.to_string(),
                            "version_id": version_id.to_string(),
                            "job_id": outcome.job.id.to_string(),
                            "job_type": "index",
                        }),
                    },
                )
                .await?;
                Ok::<_, jobs::JobError>(outcome)
            })
        }
    })
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    Ok(Json(serde_json::json!({
        "jobId": outcome.job.id,
        "created": outcome.created,
        "documentId": document_id,
        "versionId": version_id,
        "requestId": auth.request_id,
    })))
}

async fn resolve_citation_route(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<ResolveBody>,
) -> Result<Json<serde_json::Value>, RouteError> {
    let store = state
        .object_store()
        .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
    let pin = resolve_citation(
        state.pool(),
        &auth.context,
        store,
        ResolveCitationRequest {
            logical_document_id: body.logical_document_id,
            version_id: body.version_id,
            source_content_sha256: body.source_content_sha256,
            canonical_markdown_sha256: body.canonical_markdown_sha256,
            chunk_id: body.chunk_id,
            source_span_start: body.source_span_start,
            source_span_end: body.source_span_end,
            quote_local_start: body.quote_local_start,
            quote_local_end: body.quote_local_end,
            quote: body.quote,
            require_current: body.require_current,
        },
    )
    .await
    .map_err(|error| match error {
        crate::services::citation::CitationError::PermissionDenied
        | crate::services::citation::CitationError::HistoryDenied => {
            RouteError::Denied(auth.request_id.clone())
        }
        crate::services::citation::CitationError::NotFound
        | crate::services::citation::CitationError::Suppressed => {
            RouteError::NotFound(auth.request_id.clone())
        }
        crate::services::citation::CitationError::InvalidRequest
        | crate::services::citation::CitationError::IntegrityMismatch
        | crate::services::citation::CitationError::Storage
        | crate::services::citation::CitationError::ArtifactUnavailable => {
            RouteError::Validation(auth.request_id.clone(), "Citation failed validation")
        }
        crate::services::citation::CitationError::Database => {
            RouteError::Database(auth.request_id.clone())
        }
    })?;
    Ok(Json(serde_json::json!({
        "citation": {
            "citeId": pin.cite_id,
            "logicalDocumentId": pin.logical_document_id,
            "versionId": pin.version_id,
            "versionNumber": pin.version_number,
            "sourceContentSha256": pin.source_content_sha256,
            "canonicalMarkdownSha256": pin.canonical_markdown_sha256,
            "quoteSha256": pin.quote_sha256,
            "chunkId": pin.chunk_id,
            "chunkIdentitySha256": pin.chunk_identity_sha256,
            "page": pin.page,
            "slide": pin.slide,
            "sheet": pin.sheet,
            "sourceSpanStart": pin.source_span_start,
            "sourceSpanEnd": pin.source_span_end,
            "quoteLocalStart": pin.quote_local_start,
            "quoteLocalEnd": pin.quote_local_end,
            "quote": pin.quote,
            "isCurrent": pin.is_current,
            "anchor": pin.anchor,
        },
        "requestId": auth.request_id,
    })))
}

async fn list_conflicts(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
) -> Result<Json<serde_json::Value>, RouteError> {
    require_permission(&auth.context, "qa.query")
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let rows = access::list_authorized_conflicts(state.pool(), &auth.context)
        .await
        .map_err(|error| RouteError::from_access(error, &auth.request_id))?;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "id": row.get::<_, Uuid>("id"),
                "status": row.get::<_, String>("status"),
                "severity": row.get::<_, String>("severity"),
                "conflictType": row.get::<_, String>("conflict_type"),
                "claimAId": row.get::<_, Uuid>("claim_a_id"),
                "claimBId": row.get::<_, Uuid>("claim_b_id"),
                "collectionAId": row.get::<_, Uuid>("collection_a_id"),
                "collectionBId": row.get::<_, Uuid>("collection_b_id"),
                "firstDetectedAt": row.get::<_, chrono::DateTime<chrono::Utc>>("first_detected_at"),
                "resolvedAt": row.get::<_, Option<chrono::DateTime<chrono::Utc>>>("resolved_at"),
            })
        })
        .collect();
    Ok(Json(
        serde_json::json!({ "items": items, "requestId": auth.request_id }),
    ))
}

async fn get_conflict(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(conflict_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, RouteError> {
    require_permission(&auth.context, "qa.query")
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let row = access::resolve_conflict(state.pool(), &auth.context, conflict_id)
        .await
        .map_err(|error| RouteError::from_access(error, &auth.request_id))?;
    Ok(Json(serde_json::json!({
        "id": row.get::<_, Uuid>("id"),
        "status": row.get::<_, String>("status"),
        "severity": row.get::<_, String>("severity"),
        "conflictType": row.get::<_, String>("conflict_type"),
        "claimAId": row.get::<_, Uuid>("claim_a_id"),
        "claimBId": row.get::<_, Uuid>("claim_b_id"),
        "collectionAId": row.get::<_, Uuid>("collection_a_id"),
        "collectionBId": row.get::<_, Uuid>("collection_b_id"),
        "resolutionNote": row.get::<_, Option<String>>("resolution_note"),
        "resolvedAt": row.get::<_, Option<chrono::DateTime<chrono::Utc>>>("resolved_at"),
        "requestId": auth.request_id,
    })))
}

async fn get_conflict_evidence(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(conflict_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, RouteError> {
    require_permission(&auth.context, "qa.query")
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let row = access::resolve_conflict(state.pool(), &auth.context, conflict_id)
        .await
        .map_err(|error| RouteError::from_access(error, &auth.request_id))?;
    let evidence = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                txn.query(
                    "SELECT id, claim_id, evidence_role, citation_quote, created_at
                     FROM conflict_evidence
                     WHERE org_id = $1 AND conflict_id = $2
                     ORDER BY evidence_role, created_at",
                    &[&ctx.org_id(), &conflict_id],
                )
                .await
                .map_err(crate::db::error::DbError::from)
            })
        }
    })
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    let items: Vec<serde_json::Value> = evidence
        .iter()
        .map(|item| {
            serde_json::json!({
                "id": item.get::<_, Uuid>("id"),
                "claimId": item.get::<_, Uuid>("claim_id"),
                "evidenceRole": item.get::<_, String>("evidence_role"),
                "citationQuote": item.get::<_, Option<String>>("citation_quote"),
                "createdAt": item.get::<_, chrono::DateTime<chrono::Utc>>("created_at"),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "conflictId": row.get::<_, Uuid>("id"),
        "status": row.get::<_, String>("status"),
        "resolutionNote": row.get::<_, Option<String>>("resolution_note"),
        "resolvedAt": row.get::<_, Option<chrono::DateTime<chrono::Utc>>>("resolved_at"),
        "items": items,
        "requestId": auth.request_id,
    })))
}

async fn triage_conflict(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(conflict_id): Path<Uuid>,
    Json(body): Json<TriageConflictRequest>,
) -> Result<Json<serde_json::Value>, RouteError> {
    require_permission(&auth.context, "qa.query")
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let status = body.status.as_str();
    if !matches!(status, "resolved" | "accepted_exception" | "false_positive") {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "status must be resolved|accepted_exception|false_positive",
        ));
    }
    let updated = access::triage_authorized_conflict(
        state.pool(),
        &auth.context,
        conflict_id,
        status,
        body.resolution_note.as_deref(),
    )
    .await
    .map_err(|error| RouteError::from_access(error, &auth.request_id))?;
    let resource_id = conflict_id.to_string();
    audit::record(
        state.pool(),
        &auth.context,
        audit::AuditRecord {
            request_id: &auth.request_id,
            action: "conflict.triage",
            resource_type: "conflict",
            resource_id: Some(&resource_id),
            outcome: AuditOutcome::Success,
            metadata: serde_json::json!({
                "conflict_id": conflict_id.to_string(),
                "status": body.status,
            }),
        },
    )
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    Ok(Json(serde_json::json!({
        "id": updated.get::<_, Uuid>("id"),
        "status": updated.get::<_, String>("status"),
        "resolvedAt": updated.get::<_, chrono::DateTime<chrono::Utc>>("resolved_at"),
        "requestId": auth.request_id,
    })))
}

async fn load_document(
    state: &AppState,
    auth: &AuthenticatedOrg,
    document_id: Uuid,
) -> Result<crate::db::models::Document, RouteError> {
    access::resolve_document(state.pool(), &auth.context, document_id)
        .await
        .map_err(|error| RouteError::from_access(error, &auth.request_id))
}

fn document_dto(row: crate::db::models::Document) -> DocumentDto {
    DocumentDto {
        id: row.id,
        collection_id: row.collection_id,
        title: row.title,
        state: row.state.as_str().into(),
        current_version_id: row.current_version_id,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn version_dto(row: crate::db::models::DocumentVersion) -> DocumentVersionDto {
    DocumentVersionDto {
        id: row.id,
        document_id: row.document_id,
        version_number: row.version_number,
        is_current: row.is_current,
        source_content_sha256: row.content_sha256,
        effective_from: row.effective_from,
        effective_to: row.effective_to,
        change_summary: row.change_summary,
        created_at: row.created_at,
    }
}

enum RouteError {
    Denied(String),
    Validation(String, &'static str),
    NotFound(String),
    Unavailable(String),
    Database(String),
    RateLimited(crate::routes::rate_limit_guard::RateLimitRejected),
}

impl RouteError {
    fn from_db(error: DbError, request_id: &str) -> Self {
        match error {
            DbError::NotFound => Self::NotFound(request_id.to_string()),
            DbError::Config(message) if message == "collection_denied" => {
                // IDOR / collection scope → not found
                Self::NotFound(request_id.to_string())
            }
            _ => Self::Database(request_id.to_string()),
        }
    }

    fn from_access(error: AccessError, request_id: &str) -> Self {
        match error {
            AccessError::NotFound | AccessError::NotPublished => {
                Self::NotFound(request_id.to_string())
            }
            AccessError::HistoryRequired => Self::Denied(request_id.to_string()),
            AccessError::Database => Self::Database(request_id.to_string()),
        }
    }

    fn from_preview(error: preview::PreviewError, request_id: &str) -> Self {
        match error {
            preview::PreviewError::PermissionDenied | preview::PreviewError::HistoryRequired => {
                Self::Denied(request_id.to_string())
            }
            preview::PreviewError::NotFound
            | preview::PreviewError::Suppressed
            | preview::PreviewError::NotPublished => Self::NotFound(request_id.to_string()),
            preview::PreviewError::TooLarge | preview::PreviewError::ArtifactUnavailable => {
                Self::Validation(request_id.to_string(), "Preview unavailable")
            }
            _ => Self::Database(request_id.to_string()),
        }
    }

    fn from_download(error: download::DownloadError, request_id: &str) -> Self {
        match error {
            download::DownloadError::PermissionDenied
            | download::DownloadError::HistoryRequired => Self::Denied(request_id.to_string()),
            download::DownloadError::NotFound
            | download::DownloadError::Suppressed
            | download::DownloadError::NotPublished => Self::NotFound(request_id.to_string()),
            download::DownloadError::InvalidCapability
            | download::DownloadError::Replay
            | download::DownloadError::TooLarge => {
                Self::Validation(request_id.to_string(), "Download capability rejected")
            }
            download::DownloadError::NotConfigured | download::DownloadError::ObjectUnavailable => {
                Self::Unavailable(request_id.to_string())
            }
            _ => Self::Database(request_id.to_string()),
        }
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        if let Self::RateLimited(rejected) = self {
            return rejected.into_response();
        }
        let (status, code, message, request_id) = match self {
            Self::Denied(request_id) => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "Permission denied",
                request_id,
            ),
            Self::Validation(request_id, message) => (
                StatusCode::BAD_REQUEST,
                "validation_failed",
                message,
                request_id,
            ),
            Self::NotFound(request_id) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Resource not found",
                request_id,
            ),
            Self::Unavailable(request_id) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "dependency_unavailable",
                "Required dependency unavailable",
                request_id,
            ),
            Self::Database(request_id) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Request failed",
                request_id,
            ),
            Self::RateLimited(_) => unreachable!(),
        };
        (
            status,
            Json(ApiError {
                code: code.into(),
                message: message.into(),
                request_id,
                details: None,
            }),
        )
            .into_response()
    }
}
