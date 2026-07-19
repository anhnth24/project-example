//! Document citation, preview, and original-download routes.

use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::api::{ApiError, PageInfo};
use crate::auth::context::OrgContext;
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::document_versions::{self, NewSourceVersion};
use crate::db::documents::{self, NewDocument};
use crate::db::models::{Document, DocumentState, DocumentVersion, Job, JobType, PublicationState};
use crate::db::pool::with_org_txn_typed;
use crate::http::AppState;
use crate::jobs::{self, EnqueueJob, JobPayload};
use crate::routes::common::{
    db_or_404, decode_cursor, encode_cursor, page_info, parse_page_limit, parse_uuid,
    require_collection_or_404, require_permission_or_403, validate_idempotency_header,
    ListResponse, PageParams, RestError, TxnRestError,
};
use crate::services::audit::{self, SafeAuditEvent};
use crate::services::citation::{self, CitationPin};
use crate::services::deletion::{self, DeleteRequestOutcome, DeletionError};
use crate::services::download::{self, DownloadError};
use crate::services::preview::{self, PreviewError, MARKDOWN_CONTENT_TYPE};
use crate::storage::{parse_key_for_org, quarantine_key, ObjectKey, ObjectNamespace, StorageError};

const JSON_BODY_LIMIT: usize = 16 * 1024;
const DOCUMENT_CURSOR: &str = "document.created_id.v1";
const VERSION_CURSOR: &str = "document_version.number_id.v1";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/collections/{collectionId}/documents",
            get(list_collection_documents).post(create_document_from_upload),
        )
        .route(
            "/api/v1/documents/{documentId}",
            get(get_document)
                .delete(delete_document)
                .post(reindex_document),
        )
        .route(
            "/api/v1/documents/{documentId}/versions",
            get(list_document_versions),
        )
        .route(
            "/api/v1/documents/{documentId}/versions/{versionId}/citations:resolve",
            post(resolve_citation),
        )
        .route(
            "/api/v1/documents/{documentId}/versions/{versionId}/preview",
            get(markdown_preview),
        )
        .route(
            "/api/v1/documents/{documentId}/versions/{versionId}/download",
            post(authorize_download),
        )
        .route("/api/v1/documents/download/{token}", get(redeem_download))
        .route_layer(DefaultBodyLimit::max(JSON_BODY_LIMIT))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionDocumentsPath {
    collection_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentPath {
    document_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentCursor {
    created_at: chrono::DateTime<chrono::Utc>,
    id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionCursor {
    version_number: i32,
    id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateDocumentRequest {
    object_key: Option<String>,
    object_id: Option<String>,
    title: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentResponse {
    id: Uuid,
    collection_id: Uuid,
    title: String,
    state: &'static str,
    current_version_id: Option<Uuid>,
    created_by_user_id: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<Document> for DocumentResponse {
    fn from(value: Document) -> Self {
        Self {
            id: value.id,
            collection_id: value.collection_id,
            title: value.title,
            state: value.state.as_str(),
            current_version_id: value.current_version_id,
            created_by_user_id: value.created_by_user_id,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentVersionResponse {
    id: Uuid,
    document_id: Uuid,
    version_number: i32,
    parent_version_id: Option<Uuid>,
    publication_state: &'static str,
    is_current: bool,
    content_sha256: String,
    source_filename: Option<String>,
    source_content_type: Option<String>,
    byte_size: Option<i64>,
    effective_from: chrono::DateTime<chrono::Utc>,
    effective_to: Option<chrono::DateTime<chrono::Utc>>,
    change_summary: Option<String>,
    created_by_user_id: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl From<DocumentVersion> for DocumentVersionResponse {
    fn from(value: DocumentVersion) -> Self {
        Self {
            id: value.id,
            document_id: value.document_id,
            version_number: value.version_number,
            parent_version_id: value.parent_version_id,
            publication_state: publication_state_str(value.publication_state),
            is_current: value.is_current,
            content_sha256: value.content_sha256,
            source_filename: value.source_filename,
            source_content_type: value.source_content_type,
            byte_size: value.byte_size,
            effective_from: value.effective_from,
            effective_to: value.effective_to,
            change_summary: value.change_summary,
            created_by_user_id: value.created_by_user_id,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateDocumentResponse {
    document: DocumentResponse,
    version: DocumentVersionResponse,
    job_id: Uuid,
    job_status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteDocumentResponse {
    document: DocumentResponse,
    delete_requested: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobSummaryResponse {
    id: Uuid,
    status: &'static str,
}

impl From<Job> for JobSummaryResponse {
    fn from(value: Job) -> Self {
        Self {
            id: value.id,
            status: value.status.as_str(),
        }
    }
}

#[derive(Debug)]
struct SourceObjectMetadata {
    document_id: Uuid,
    version_id: Uuid,
    content_sha256: String,
    byte_size: i64,
    source_filename: Option<String>,
    source_content_type: Option<String>,
}

#[derive(Debug)]
struct ValidCreateDocument {
    object_key: ObjectKey,
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionPath {
    document_id: Uuid,
    version_id: Uuid,
}

#[derive(Debug, Deserialize)]
struct TokenPath {
    token: String,
}

async fn list_collection_documents(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<CollectionDocumentsPath>,
    query: Result<Query<PageParams>, QueryRejection>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let collection_id = parse_uuid(&path.collection_id, &request_id)?;
    let Query(params) =
        query.map_err(|_| RestError::validation("query string is invalid", &request_id))?;
    let page_limit = parse_page_limit(&params, &request_id)?;
    let cursor_key = state
        .download_capability_key()
        .ok_or_else(|| RestError::service_unavailable(&request_id))?;
    let after: Option<DocumentCursor> = decode_cursor(
        cursor_key,
        DOCUMENT_CURSOR,
        params.cursor.as_deref(),
        &request_id,
    )?;

    let rows = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_permission_or_403(&ctx, "qa.query", &request_id)?;
                require_collection_or_404(&ctx, collection_id, &request_id)?;
                let after = after.map(|cursor| (cursor.created_at, cursor.id));
                let documents = documents::list_by_collection(
                    txn,
                    &ctx,
                    collection_id,
                    after,
                    page_limit.fetch_size,
                )
                .await?;
                Ok::<_, TxnRestError>(documents)
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;

    let (items, page_info) = document_page(cursor_key, rows, page_limit.page_size)?;
    Ok(Json(ListResponse { items, page_info }).into_response())
}

async fn create_document_from_upload(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<CollectionDocumentsPath>,
    body: Result<Json<CreateDocumentRequest>, JsonRejection>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let collection_id = parse_uuid(&path.collection_id, &request_id)?;
    let Json(body) =
        body.map_err(|_| RestError::validation("request body is invalid", &request_id))?;
    let input = validate_create_document(body, auth.context.org_id(), &request_id)?;

    with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_permission_or_403(&ctx, "doc.upload", &request_id)?;
                require_collection_or_404(&ctx, collection_id, &request_id)?;
                crate::db::collections::get_by_id(txn, &ctx, collection_id).await?;
                Ok::<_, TxnRestError>(())
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;

    let storage = state
        .object_store()
        .ok_or_else(|| RestError::service_unavailable(&request_id))?;
    let metadata = storage
        .head_metadata(auth.context.org_id(), &input.object_key)
        .await
        .map_err(|error| storage_error_for_create(error, &request_id))?;
    let source = parse_source_metadata(&metadata, auth.context.org_id(), &request_id)?;
    let object_key = input.object_key.as_str();
    let title = input.title;

    let (document, version, job) = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_permission_or_403(&ctx, "doc.upload", &request_id)?;
                require_collection_or_404(&ctx, collection_id, &request_id)?;
                crate::db::collections::get_by_id(txn, &ctx, collection_id).await?;
                let document = documents::insert(
                    txn,
                    &ctx,
                    NewDocument {
                        id: source.document_id,
                        collection_id,
                        title: &title,
                    },
                )
                .await?;
                let version = document_versions::insert_source_version(
                    txn,
                    &ctx,
                    NewSourceVersion {
                        id: source.version_id,
                        document_id: document.id,
                        original_object_key: &object_key,
                        content_sha256: &source.content_sha256,
                        source_filename: source.source_filename.as_deref(),
                        source_content_type: source.source_content_type.as_deref(),
                        byte_size: source.byte_size,
                    },
                )
                .await?;
                let outcome = jobs::enqueue_within_txn(
                    txn,
                    &ctx,
                    EnqueueJob::new(
                        JobType::Convert,
                        JobPayload {
                            document_id: Some(document.id),
                            version_id: Some(version.id),
                            ..JobPayload::default()
                        },
                        format!("convert:{}", version.id),
                    ),
                )
                .await
                .map_err(|_| RestError::internal(&request_id))?;
                Ok::<_, TxnRestError>((document, version, outcome.job))
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;

    Ok((
        StatusCode::CREATED,
        Json(CreateDocumentResponse {
            document: DocumentResponse::from(document),
            version: DocumentVersionResponse::from(version),
            job_id: job.id,
            job_status: job.status.as_str(),
        }),
    )
        .into_response())
}

async fn get_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<DocumentPath>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let document_id = parse_plain_document_id(&path.document_id, &request_id)?;
    let document = load_visible_document(state, &auth, document_id).await?;
    Ok(Json(DocumentResponse::from(document)).into_response())
}

async fn delete_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    headers: HeaderMap,
    Path(path): Path<DocumentPath>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    validate_idempotency_header(&headers, &request_id)?;
    let document_id = parse_plain_document_id(&path.document_id, &request_id)?;
    with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_permission_or_403(&ctx, "doc.delete", &request_id)?;
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                require_collection_or_404(&ctx, document.collection_id, &request_id)?;
                Ok::<_, TxnRestError>(())
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;

    let outcome = deletion::request_delete(state.pool(), &auth.context, document_id)
        .await
        .map_err(|error| deletion_error(error, &request_id))?;
    match outcome {
        DeleteRequestOutcome::Requested(document) => Ok((
            StatusCode::ACCEPTED,
            Json(DeleteDocumentResponse {
                document: DocumentResponse::from(document),
                delete_requested: true,
            }),
        )
            .into_response()),
        DeleteRequestOutcome::AlreadyRequested(document) => Ok(Json(DeleteDocumentResponse {
            document: DocumentResponse::from(document),
            delete_requested: false,
        })
        .into_response()),
    }
}

async fn reindex_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<DocumentPath>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let document_id = parse_document_action_id(&path.document_id, "reindex", &request_id)?;
    let job = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_reindex_permission(&ctx, &request_id)?;
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                authorize_visible_document(&ctx, &document, &request_id)?;
                let version_id = document.current_version_id.ok_or_else(|| {
                    RestError::conflict("document has no current version", &request_id)
                })?;
                let outcome = jobs::enqueue_within_txn(
                    txn,
                    &ctx,
                    EnqueueJob::new(
                        JobType::Index,
                        JobPayload {
                            document_id: Some(document.id),
                            version_id: Some(version_id),
                            ..JobPayload::default()
                        },
                        format!("index:{version_id}"),
                    ),
                )
                .await
                .map_err(|_| RestError::internal(&request_id))?;
                Ok::<_, TxnRestError>(outcome.job)
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;
    Ok((StatusCode::ACCEPTED, Json(JobSummaryResponse::from(job))).into_response())
}

async fn list_document_versions(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<DocumentPath>,
    query: Result<Query<PageParams>, QueryRejection>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let document_id = parse_uuid(&path.document_id, &request_id)?;
    let Query(params) =
        query.map_err(|_| RestError::validation("query string is invalid", &request_id))?;
    let page_limit = parse_page_limit(&params, &request_id)?;
    let cursor_key = state
        .download_capability_key()
        .ok_or_else(|| RestError::service_unavailable(&request_id))?;
    let after: Option<VersionCursor> = decode_cursor(
        cursor_key,
        VERSION_CURSOR,
        params.cursor.as_deref(),
        &request_id,
    )?;

    let rows = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_permission_or_403(&ctx, "qa.query", &request_id)?;
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                authorize_visible_document(&ctx, &document, &request_id)?;
                let after = after.map(|cursor| (cursor.version_number, cursor.id));
                let versions = document_versions::list_by_document_paginated(
                    txn,
                    &ctx,
                    document_id,
                    after,
                    page_limit.fetch_size,
                )
                .await?;
                Ok::<_, TxnRestError>(versions)
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;

    let (items, page_info) = version_page(cursor_key, rows, page_limit.page_size)?;
    Ok(Json(ListResponse { items, page_info }).into_response())
}

async fn resolve_citation(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<VersionPath>,
    Json(pin): Json<CitationPin>,
) -> Result<Response, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    if require_permission(&auth.context, "qa.query").is_err() {
        let _ = audit_download(
            &state,
            &auth.context,
            "document.download.mint",
            "deny",
            Some(path.version_id.to_string()),
            &request_id,
            serde_json::json!({
                "reason": "permission_denied",
                "documentId": path.document_id.to_string(),
                "versionId": path.version_id.to_string()
            }),
        )
        .await;
        return Err(DocumentRouteError::forbidden(request_id.clone()));
    }
    if path.document_id != pin.document_id || path.version_id != pin.version_id {
        return Err(DocumentRouteError::validation(
            "Citation pin does not match the requested document version",
            request_id,
        ));
    }
    let resolved = citation::resolve_citation(state.pool(), &auth.context, pin)
        .await
        .map_err(|error| DocumentRouteError::citation(error, request_id.clone()))?;
    Ok(Json(resolved).into_response())
}

async fn markdown_preview(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<VersionPath>,
) -> Result<Response, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    require_permission(&auth.context, "qa.query")
        .map_err(|_| DocumentRouteError::forbidden(request_id.clone()))?;
    let storage = state
        .object_store()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let preview = preview::fetch_markdown_preview(
        state.pool(),
        storage,
        &auth.context,
        path.document_id,
        path.version_id,
    )
    .await
    .map_err(|error| DocumentRouteError::preview(error, request_id.clone()))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(MARKDOWN_CONTENT_TYPE),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok((headers, Body::from(preview.bytes)).into_response())
}

async fn authorize_download(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<VersionPath>,
) -> Result<Response, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    require_permission(&auth.context, "qa.query")
        .map_err(|_| DocumentRouteError::forbidden(request_id.clone()))?;
    let storage = state
        .object_store()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let key = state
        .download_capability_key()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let capability = match download::authorize_download(
        state.pool(),
        storage,
        &auth.context,
        path.document_id,
        path.version_id,
        key,
        Utc::now(),
    )
    .await
    {
        Ok(capability) => capability,
        Err(error) => {
            let _ = audit_download(
                &state,
                &auth.context,
                "document.download.mint",
                download_audit_outcome(&error),
                Some(path.version_id.to_string()),
                &request_id,
                serde_json::json!({
                    "reason": download_audit_reason(&error),
                    "documentId": path.document_id.to_string(),
                    "versionId": path.version_id.to_string()
                }),
            )
            .await;
            return Err(DocumentRouteError::download(error, request_id.clone()));
        }
    };
    let _ = audit_download(
        &state,
        &auth.context,
        "document.download.mint",
        "success",
        Some(path.version_id.to_string()),
        &request_id,
        serde_json::json!({
            "documentId": path.document_id.to_string(),
            "versionId": path.version_id.to_string(),
            "byteSize": capability.byte_size
        }),
    )
    .await;
    Ok(Json(capability).into_response())
}

async fn redeem_download(
    State(state): State<Arc<AppState>>,
    Path(path): Path<TokenPath>,
) -> Result<Response, DocumentRouteError> {
    let request_id = Uuid::new_v4().to_string();
    let storage = state
        .object_store()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let key = state
        .download_capability_key()
        .ok_or_else(|| DocumentRouteError::service_unavailable(request_id.clone()))?;
    let stream = download::redeem_download(
        state.pool(),
        storage,
        key,
        state.consumed_download_nonces(),
        &path.token,
        &request_id,
        Utc::now(),
    )
    .await
    .map_err(|error| DocumentRouteError::download(error, request_id.clone()))?;
    let disposition = download::content_disposition_value(&stream.filename);
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&stream.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok((headers, Body::from(stream.bytes)).into_response())
}

async fn audit_download(
    state: &Arc<AppState>,
    ctx: &OrgContext,
    action: &'static str,
    outcome: &'static str,
    resource_id: Option<String>,
    request_id: &str,
    metadata: serde_json::Value,
) -> Result<(), crate::db::error::DbError> {
    audit::record_audit_event(
        state.pool(),
        ctx,
        SafeAuditEvent {
            action,
            resource_type: "document_version",
            resource_id,
            outcome,
            request_id: request_id.into(),
            metadata,
        },
    )
    .await
}

fn download_audit_outcome(error: &DownloadError) -> &'static str {
    match error {
        DownloadError::NotFound
        | DownloadError::InvalidToken
        | DownloadError::Expired
        | DownloadError::Replay => "deny",
        DownloadError::CapabilityUnavailable
        | DownloadError::Db(_)
        | DownloadError::Storage(_)
        | DownloadError::Integrity => "error",
    }
}

fn download_audit_reason(error: &DownloadError) -> &'static str {
    match error {
        DownloadError::NotFound => "source_not_found",
        DownloadError::CapabilityUnavailable => "capability_unavailable",
        DownloadError::InvalidToken => "invalid_token",
        DownloadError::Expired => "expired",
        DownloadError::Replay => "replay",
        DownloadError::Db(_) => "database_error",
        DownloadError::Storage(_) => "storage_error",
        DownloadError::Integrity => "integrity_failed",
    }
}

async fn load_visible_document(
    state: Arc<AppState>,
    auth: &AuthenticatedOrg,
    document_id: Uuid,
) -> Result<Document, RestError> {
    let request_id = auth.request_id.clone();
    with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_permission_or_403(&ctx, "qa.query", &request_id)?;
                let document = documents::get_by_id(txn, &ctx, document_id).await?;
                authorize_visible_document(&ctx, &document, &request_id)?;
                Ok::<_, TxnRestError>(document)
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))
}

fn authorize_visible_document(
    ctx: &OrgContext,
    document: &Document,
    request_id: &str,
) -> Result<(), RestError> {
    require_collection_or_404(ctx, document.collection_id, request_id)?;
    if document.deleted_at.is_some() || document.state == DocumentState::Purged {
        return Err(RestError::not_found(request_id));
    }
    Ok(())
}

fn require_reindex_permission(ctx: &OrgContext, request_id: &str) -> Result<(), RestError> {
    if ctx.has_permission("doc.publish") || ctx.has_permission("doc.upload") {
        Ok(())
    } else {
        Err(RestError::forbidden(request_id))
    }
}

fn parse_plain_document_id(value: &str, request_id: &str) -> Result<Uuid, RestError> {
    if value.contains(':') {
        return Err(RestError::validation("document id is invalid", request_id));
    }
    parse_uuid(value, request_id)
}

fn parse_document_action_id(
    value: &str,
    action: &str,
    request_id: &str,
) -> Result<Uuid, RestError> {
    let suffix = format!(":{action}");
    let Some(document_id) = value.strip_suffix(&suffix) else {
        return Err(RestError::validation(
            "document action route is invalid",
            request_id,
        ));
    };
    if document_id.contains(':') {
        return Err(RestError::validation(
            "document action route is invalid",
            request_id,
        ));
    }
    parse_uuid(document_id, request_id)
}

fn validate_create_document(
    body: CreateDocumentRequest,
    org_id: Uuid,
    request_id: &str,
) -> Result<ValidCreateDocument, RestError> {
    let title = body.title.trim().to_string();
    if title.is_empty() || title.len() > 512 {
        return Err(RestError::validation(
            "title must be between 1 and 512 bytes",
            request_id,
        ));
    }
    let object_key = match (body.object_key, body.object_id) {
        (Some(_), Some(_)) | (None, None) => {
            return Err(RestError::validation(
                "exactly one of objectKey or objectId is required",
                request_id,
            ));
        }
        (Some(raw), None) => {
            let key = parse_key_for_org(&raw, org_id).map_err(|error| match error {
                StorageError::KeyOrgMismatch => RestError::not_found(request_id),
                StorageError::InvalidKey | StorageError::MissingScope => {
                    RestError::validation("objectKey is invalid", request_id)
                }
                _ => RestError::internal(request_id),
            })?;
            if key.namespace() != ObjectNamespace::Quarantine {
                return Err(RestError::validation(
                    "objectKey must reference a quarantine object",
                    request_id,
                ));
            }
            key
        }
        (None, Some(raw)) => {
            let object_id = Uuid::parse_str(&raw)
                .map_err(|_| RestError::validation("objectId is invalid", request_id))?;
            quarantine_key(org_id, object_id, None)
                .map_err(|_| RestError::validation("objectId is invalid", request_id))?
        }
    };
    Ok(ValidCreateDocument { object_key, title })
}

fn parse_source_metadata(
    metadata: &std::collections::HashMap<String, String>,
    org_id: Uuid,
    request_id: &str,
) -> Result<SourceObjectMetadata, RestError> {
    let stored_org = metadata
        .get("org-id")
        .ok_or_else(|| RestError::not_found(request_id))?;
    let stored_org = Uuid::parse_str(stored_org).map_err(|_| RestError::not_found(request_id))?;
    if stored_org != org_id {
        return Err(RestError::not_found(request_id));
    }
    let content_sha256 = metadata
        .get("content-sha256")
        .filter(|value| is_lower_sha256(value))
        .cloned()
        .ok_or_else(|| {
            RestError::validation("stored content metadata is incomplete", request_id)
        })?;
    let byte_size_u64 = metadata
        .get("content-length-bytes")
        .ok_or_else(|| RestError::validation("stored content metadata is incomplete", request_id))?
        .parse::<u64>()
        .map_err(|_| RestError::validation("stored content metadata is invalid", request_id))?;
    let byte_size = i64::try_from(byte_size_u64)
        .map_err(|_| RestError::validation("stored content metadata is invalid", request_id))?;
    let canonical_format = metadata
        .get("canonical-format")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            RestError::validation("stored content metadata is incomplete", request_id)
        })?;
    let source_content_type = metadata
        .get("content-type")
        .cloned()
        .or_else(|| Some(format!("application/x-markhand-{canonical_format}")));
    let document_id = parse_optional_metadata_uuid(metadata, "document-id", request_id)?
        .unwrap_or_else(Uuid::new_v4);
    let version_id = parse_optional_metadata_uuid(metadata, "version-id", request_id)?
        .unwrap_or_else(Uuid::new_v4);
    Ok(SourceObjectMetadata {
        document_id,
        version_id,
        content_sha256,
        byte_size,
        source_filename: metadata.get("original-filename").cloned(),
        source_content_type,
    })
}

fn parse_optional_metadata_uuid(
    metadata: &std::collections::HashMap<String, String>,
    key: &str,
    request_id: &str,
) -> Result<Option<Uuid>, RestError> {
    metadata
        .get(key)
        .map(|value| {
            Uuid::parse_str(value).map_err(|_| {
                RestError::validation("stored identity metadata is invalid", request_id)
            })
        })
        .transpose()
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn storage_error_for_create(error: StorageError, request_id: &str) -> RestError {
    match error {
        StorageError::NotFound
        | StorageError::KeyOrgMismatch
        | StorageError::OwnershipConflict
        | StorageError::MissingScope => RestError::not_found(request_id),
        StorageError::InvalidKey => RestError::validation("objectKey is invalid", request_id),
        StorageError::ConfigInvalid
        | StorageError::ConfigMissingCredentials
        | StorageError::Transport
        | StorageError::Backend
        | StorageError::PreconditionFailed
        | StorageError::CollectionMismatch => RestError::service_unavailable(request_id),
    }
}

fn deletion_error(error: DeletionError, request_id: &str) -> RestError {
    match error {
        DeletionError::Db(db) => db_or_404(db, request_id),
        DeletionError::UnexpectedState(_) => RestError::conflict(
            "document cannot be deleted in its current state",
            request_id,
        ),
        DeletionError::Job(_) => RestError::internal(request_id),
        DeletionError::Storage(_) => RestError::service_unavailable(request_id),
        DeletionError::InvalidPayload | DeletionError::InvalidCheckpoint => {
            RestError::internal(request_id)
        }
        DeletionError::JobTimedOut => RestError::service_unavailable(request_id),
    }
}

fn document_page(
    cursor_key: &crate::services::download::CapabilityKey,
    mut rows: Vec<Document>,
    page_size: usize,
) -> Result<(Vec<DocumentResponse>, PageInfo), RestError> {
    let has_more = rows.len() > page_size;
    if has_more {
        rows.truncate(page_size);
    }
    let next_cursor = if has_more {
        rows.last()
            .map(|item| {
                encode_cursor(
                    cursor_key,
                    DOCUMENT_CURSOR,
                    &DocumentCursor {
                        created_at: item.created_at,
                        id: item.id,
                    },
                )
            })
            .transpose()?
    } else {
        None
    };
    let items = rows.into_iter().map(DocumentResponse::from).collect();
    Ok((items, page_info(next_cursor)))
}

fn version_page(
    cursor_key: &crate::services::download::CapabilityKey,
    mut rows: Vec<DocumentVersion>,
    page_size: usize,
) -> Result<(Vec<DocumentVersionResponse>, PageInfo), RestError> {
    let has_more = rows.len() > page_size;
    if has_more {
        rows.truncate(page_size);
    }
    let next_cursor = if has_more {
        rows.last()
            .map(|item| {
                encode_cursor(
                    cursor_key,
                    VERSION_CURSOR,
                    &VersionCursor {
                        version_number: item.version_number,
                        id: item.id,
                    },
                )
            })
            .transpose()?
    } else {
        None
    };
    let items = rows
        .into_iter()
        .map(DocumentVersionResponse::from)
        .collect();
    Ok((items, page_info(next_cursor)))
}

fn publication_state_str(value: PublicationState) -> &'static str {
    match value {
        PublicationState::Draft => "draft",
        PublicationState::Published => "published",
    }
}

struct DocumentRouteError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    request_id: String,
}

impl DocumentRouteError {
    fn validation(message: &'static str, request_id: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "validation_failed",
            message,
            request_id,
        }
    }

    fn forbidden(request_id: String) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "permission_denied",
            message: "Permission denied",
            request_id,
        }
    }

    fn not_found(request_id: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "Document source was not found",
            request_id,
        }
    }

    fn internal(request_id: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "Document operation failed",
            request_id,
        }
    }

    fn service_unavailable(request_id: String) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "dependency_unavailable",
            message: "A required document service is unavailable",
            request_id,
        }
    }

    fn citation(error: citation::CitationError, request_id: String) -> Self {
        match error {
            citation::CitationError::NotFound => Self::not_found(request_id),
            citation::CitationError::Db(_) => Self::internal(request_id),
        }
    }

    fn preview(error: PreviewError, request_id: String) -> Self {
        match error {
            PreviewError::NotFound
            | PreviewError::Storage(
                StorageError::NotFound
                | StorageError::InvalidKey
                | StorageError::KeyOrgMismatch
                | StorageError::OwnershipConflict
                | StorageError::MissingScope,
            ) => Self::not_found(request_id),
            PreviewError::Db(_) | PreviewError::Storage(_) | PreviewError::Integrity => {
                Self::internal(request_id)
            }
        }
    }

    fn download(error: DownloadError, request_id: String) -> Self {
        match error {
            DownloadError::NotFound
            | DownloadError::InvalidToken
            | DownloadError::Expired
            | DownloadError::Replay
            | DownloadError::Storage(
                StorageError::NotFound
                | StorageError::InvalidKey
                | StorageError::KeyOrgMismatch
                | StorageError::OwnershipConflict
                | StorageError::MissingScope,
            ) => Self::not_found(request_id),
            DownloadError::CapabilityUnavailable => Self::service_unavailable(request_id),
            DownloadError::Db(_) | DownloadError::Storage(_) | DownloadError::Integrity => {
                Self::internal(request_id)
            }
        }
    }
}

impl IntoResponse for DocumentRouteError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiError {
                code: self.code.into(),
                message: self.message.into(),
                request_id: self.request_id,
                details: None,
            }),
        )
            .into_response()
    }
}
