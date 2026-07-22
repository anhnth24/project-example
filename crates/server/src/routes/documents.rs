//! Document REST routes: R04 catalog + R02 citation/preview/download.
//!
//! Upload intake remains `POST /api/v1/uploads`. Publish uses
//! `markhand_publish_document_version` plus same-txn reindex enqueue.

use std::fmt;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::{
    decode_cursor, encode_cursor, ApiError, ApiRejection, AppJson, AppPath, AppQuery,
    ConflictEvidenceResponse, ConflictResponse, CreatedAtIdCursor, DocumentResponse,
    DocumentVersionResponse, ListResponse, PageInfo, PageParams, ReindexResponse,
    VersionDiffResponse, VersionNumberIdCursor,
};
use crate::auth::middleware::AuthenticatedOrg;
use crate::db::api_idempotency::{self, IdempotencyScope};
use crate::db::conflicts;
use crate::db::conflicts::TriageConflict;
use crate::db::document_versions;
use crate::db::documents::{self as documents_repo, DocumentListPage};
use crate::db::download_capabilities::DownloadPurpose;
use crate::db::models::{ConflictStatus, DocumentState};
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::routes::common::{
    deny_or_not_found, document_response, load_document_authorized, map_db, read_idempotency_key,
    require_perm, version_response,
};
use crate::services::citation::{self, CitationError, CitationResolveRequest, StableCitation};
use crate::services::deletion::{self, DeleteRequestOutcome, DeletionError};
use crate::services::download::{
    self, CapabilitySigner, DownloadError, DEFAULT_CAPABILITY_TTL, MAX_CAPABILITY_TTL,
};
use crate::services::indexing::{self, CatalogIndexError};
use crate::services::preview::{self, PreviewError};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/documents", get(list_documents))
        .route(
            "/api/v1/documents/{document_id}",
            get(get_document).delete(delete_document),
        )
        .route(
            "/api/v1/documents/{document_id}/reindex",
            post(reindex_document),
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
            "/api/v1/documents/{document_id}/versions/{left_version_id}/diff/{right_version_id}",
            get(diff_versions),
        )
        .route("/api/v1/conflicts", get(list_conflicts))
        .route("/api/v1/conflicts/{conflict_id}", get(get_conflict))
        .route(
            "/api/v1/conflicts/{conflict_id}/triage",
            post(triage_conflict),
        )
        .route(
            "/api/v1/conflicts/{conflict_id}/evidence",
            get(list_conflict_evidence),
        )
        .route("/api/v1/citations/resolve", post(resolve_citations))
        .route(
            "/api/v1/documents/{document_id}/versions/{version_id}/preview",
            get(preview_markdown),
        )
        .route(
            "/api/v1/documents/{document_id}/versions/{version_id}/download-capabilities",
            post(mint_download_capability),
        )
        .route(
            "/api/v1/download-capabilities/redeem",
            post(redeem_download_capability),
        )
}

/// Wire token field: serializes normally, Debug never prints the raw secret.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
struct RedactedToken(String);

impl fmt::Debug for RedactedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveCitationsBody {
    citations: Vec<ResolveCitationItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveCitationItem {
    chunk_id: Uuid,
    #[serde(default)]
    expected_version_id: Option<Uuid>,
    #[serde(default)]
    expected_document_id: Option<Uuid>,
    #[serde(default)]
    expected_content_sha256: Option<String>,
    #[serde(default)]
    expected_quote: Option<String>,
    #[serde(default)]
    expected_span_start: Option<usize>,
    #[serde(default)]
    expected_span_end: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StableCitationResponse {
    org_id: Uuid,
    logical_document_id: Uuid,
    version_id: Uuid,
    version_number: i32,
    content_sha256: String,
    chunk_id: Uuid,
    chunk_identity_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slide: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sheet: Option<String>,
    span_start: usize,
    span_end: usize,
    quote: String,
    effective_from: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_to: Option<chrono::DateTime<chrono::Utc>>,
    is_current: bool,
    heading: String,
}

impl From<StableCitation> for StableCitationResponse {
    fn from(value: StableCitation) -> Self {
        Self {
            org_id: value.org_id,
            logical_document_id: value.logical_document_id,
            version_id: value.version_id,
            version_number: value.version_number,
            content_sha256: value.content_sha256,
            chunk_id: value.chunk_id,
            chunk_identity_sha256: value.chunk_identity_sha256,
            page: value.page,
            slide: value.slide,
            sheet: value.sheet,
            span_start: value.span_start,
            span_end: value.span_end,
            quote: value.quote,
            effective_from: value.effective_from,
            effective_to: value.effective_to,
            is_current: value.is_current,
            heading: value.heading,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveCitationsResponse {
    citations: Vec<StableCitationResponse>,
    request_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewResponse {
    document_id: Uuid,
    version_id: Uuid,
    version_number: i32,
    content_sha256: String,
    markdown_sha256: String,
    is_current: bool,
    markdown: String,
    request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MintDownloadBody {
    purpose: String,
    #[serde(default)]
    ttl_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MintDownloadResponse {
    capability_id: Uuid,
    token: RedactedToken,
    purpose: String,
    document_id: Uuid,
    version_id: Uuid,
    expires_at: chrono::DateTime<chrono::Utc>,
    request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RedeemDownloadBody {
    token: RedactedToken,
}

async fn resolve_citations(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<ResolveCitationsBody>,
) -> Result<Json<ResolveCitationsResponse>, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    if body.citations.is_empty() || body.citations.len() > 40 {
        return Err(DocumentRouteError::validation(
            "citations must contain 1..=40 items",
            request_id,
        ));
    }
    let requests: Vec<CitationResolveRequest> = body
        .citations
        .into_iter()
        .map(|item| CitationResolveRequest {
            chunk_id: item.chunk_id,
            expected_version_id: item.expected_version_id,
            expected_document_id: item.expected_document_id,
            expected_content_sha256: item.expected_content_sha256,
            expected_quote: item.expected_quote,
            expected_span_start: item.expected_span_start,
            expected_span_end: item.expected_span_end,
        })
        .collect();
    let storage = state
        .object_store()
        .ok_or_else(|| DocumentRouteError::Citation(CitationError::Storage, request_id.clone()))?;
    let citations = citation::resolve_citations(
        state.pool(),
        storage,
        auth.context.org_id(),
        auth.context.user_id(),
        &requests,
    )
    .await
    .map_err(|error| DocumentRouteError::Citation(error, request_id.clone()))?;
    Ok(Json(ResolveCitationsResponse {
        citations: citations.into_iter().map(Into::into).collect(),
        request_id,
    }))
}

async fn preview_markdown(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath((document_id, version_id)): AppPath<(Uuid, Uuid)>,
) -> Result<Json<PreviewResponse>, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    let storage = state.object_store().ok_or_else(|| {
        DocumentRouteError::Preview(PreviewError::StorageUnavailable, request_id.clone())
    })?;
    let preview = preview::fetch_trusted_markdown(
        state.pool(),
        storage,
        auth.context.org_id(),
        auth.context.user_id(),
        document_id,
        version_id,
    )
    .await
    .map_err(|error| DocumentRouteError::Preview(error, request_id.clone()))?;
    Ok(Json(PreviewResponse {
        document_id: preview.document_id,
        version_id: preview.version_id,
        version_number: preview.version_number,
        content_sha256: preview.content_sha256,
        markdown_sha256: preview.markdown_sha256,
        is_current: preview.is_current,
        markdown: preview.markdown,
        request_id,
    }))
}

async fn mint_download_capability(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath((document_id, version_id)): AppPath<(Uuid, Uuid)>,
    Json(body): Json<MintDownloadBody>,
) -> Result<Json<MintDownloadResponse>, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    let purpose = DownloadPurpose::parse(&body.purpose).map_err(|_| {
        DocumentRouteError::Download(DownloadError::InvalidPurpose, request_id.clone())
    })?;
    let ttl = body
        .ttl_secs
        .map(std::time::Duration::from_secs)
        .unwrap_or(DEFAULT_CAPABILITY_TTL);
    if ttl > MAX_CAPABILITY_TTL {
        return Err(DocumentRouteError::Download(
            DownloadError::InvalidTtl,
            request_id,
        ));
    }
    let signer = capability_signer(&state)
        .map_err(|error| DocumentRouteError::Download(error, request_id.clone()))?;
    let minted = download::mint_download_capability(
        state.pool(),
        &signer,
        &download::MintDownloadCapabilityRequest {
            org_id: auth.context.org_id(),
            user_id: auth.context.user_id(),
            document_id,
            version_id,
            purpose,
            ttl,
        },
    )
    .await
    .map_err(|error| DocumentRouteError::Download(error, request_id.clone()))?;
    Ok(Json(MintDownloadResponse {
        capability_id: minted.capability_id,
        token: RedactedToken(minted.token.expose().to_string()),
        purpose: minted.purpose.as_str().into(),
        document_id: minted.document_id,
        version_id: minted.version_id,
        expires_at: minted.expires_at,
        request_id,
    }))
}

async fn redeem_download_capability(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<RedeemDownloadBody>,
) -> Result<Response, DocumentRouteError> {
    let request_id = auth.request_id.clone();
    if body.token.0.is_empty() || body.token.0.len() > 512 {
        return Err(DocumentRouteError::Download(
            DownloadError::InvalidToken,
            request_id,
        ));
    }
    let storage = state.object_store().ok_or_else(|| {
        DocumentRouteError::Download(DownloadError::StorageUnavailable, request_id.clone())
    })?;
    let signer = capability_signer(&state)
        .map_err(|error| DocumentRouteError::Download(error, request_id.clone()))?;
    let artifact = download::redeem_download_capability(
        state.pool(),
        storage,
        &signer,
        state.download_budget(),
        auth.context.org_id(),
        auth.context.user_id(),
        &body.token.0,
    )
    .await
    .map_err(|error| DocumentRouteError::Download(error, request_id.clone()))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&artifact.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::HeaderName::from_static("x-content-sha256"),
        HeaderValue::from_str(&artifact.content_sha256)
            .unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(
        header::HeaderName::from_static("x-request-id"),
        HeaderValue::from_str(&request_id).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    // Defense: never surface object keys; Content-Disposition uses safe filename only.
    if let Some(filename) = artifact.filename.as_deref() {
        let safe = filename
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>();
        if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\"")) {
            headers.insert(header::CONTENT_DISPOSITION, value);
        }
    }
    // Body owns the download budget permit until Hyper finishes / cancels / drops it.
    let mut response = Response::new(Body::new(artifact.body));
    *response.status_mut() = StatusCode::OK;
    *response.headers_mut() = headers;
    Ok(response)
}

fn capability_signer(state: &AppState) -> Result<CapabilitySigner, DownloadError> {
    CapabilitySigner::from_auth_signing_key(state.runtime().config().auth().signing_key.as_ref())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    collection_id: Option<Uuid>,
    #[serde(default)]
    include_deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConflictListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TriageBody {
    status: String,
    #[serde(default)]
    resolution_note: Option<String>,
    #[serde(default)]
    resolution_version_a_id: Option<Uuid>,
    #[serde(default)]
    resolution_version_b_id: Option<Uuid>,
}

async fn list_documents(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppQuery(query): AppQuery<DocumentListQuery>,
) -> Result<Json<ListResponse<DocumentResponse>>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let page = PageParams::from_query(query.limit, query.cursor, &request_id)?;
    if let Some(collection_id) = query.collection_id {
        if !auth.context.allows_collection(collection_id) {
            return Err(deny_or_not_found(&request_id));
        }
    }
    let after = match page.cursor.as_deref() {
        Some(raw) => Some(decode_cursor::<CreatedAtIdCursor>(raw).map_err(|message| {
            ApiRejection::validation(message, &request_id)
                .with_details(serde_json::json!({ "field": "cursor" }))
        })?),
        None => None,
    };
    let allowed: Vec<Uuid> = auth
        .context
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect();
    let fetch_limit = i64::from(page.limit) + 1;
    let mut rows = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let collection_id = query.collection_id;
        let include_deleted = query.include_deleted;
        let after_created_at = after.as_ref().map(|cursor| cursor.created_at);
        let after_id = after.as_ref().map(|cursor| cursor.id);
        move |txn| {
            Box::pin(async move {
                documents_repo::list_page(
                    txn,
                    &ctx,
                    &allowed,
                    DocumentListPage {
                        collection_id,
                        include_deleted,
                        limit: fetch_limit,
                        after_created_at,
                        after_id,
                    },
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| map_db(error, &request_id))?;
    let page_info = page_info_from_created(&mut rows, page.limit);
    Ok(Json(ListResponse {
        items: rows
            .into_iter()
            .map(|row| document_response(row, request_id.clone()))
            .collect(),
        page_info,
        request_id,
    }))
}

async fn get_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(document_id): AppPath<Uuid>,
) -> Result<Json<DocumentResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let document =
        load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    Ok(Json(document_response(document, request_id)))
}

async fn delete_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(document_id): AppPath<Uuid>,
) -> Result<Json<DocumentResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, "doc.delete", &request_id)?;
    let document =
        load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    if matches!(
        document.state,
        DocumentState::Tombstoned | DocumentState::Purged
    ) {
        return Ok(Json(document_response(document, request_id)));
    }
    let outcome = deletion::request_delete(state.pool(), &auth.context, document_id)
        .await
        .map_err(|error| map_deletion(error, &request_id))?;
    let document = match outcome {
        DeleteRequestOutcome::Requested(document)
        | DeleteRequestOutcome::AlreadyRequested(document) => document,
    };
    Ok(Json(document_response(document, request_id)))
}

async fn reindex_document(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(document_id): AppPath<Uuid>,
    headers: HeaderMap,
) -> Result<Json<ReindexResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, "doc.publish", &request_id)?;
    let client_key = read_idempotency_key(&headers, &request_id)?;
    let _ = load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    let request_hash = api_idempotency::hash_request_parts(&[b"reindex", document_id.as_bytes()]);

    // One transaction: claim/validate key → lock/enqueue → finalize. Losers block
    // on FOR UPDATE and replay the exact original (no cross-doc key reuse).
    if let Some(key) = client_key {
        let body = with_org_txn(state.pool(), &auth.context, {
            let ctx = auth.context.clone();
            let request_id = request_id.clone();
            move |txn| {
                Box::pin(async move {
                    match api_idempotency::claim_or_replay(
                        txn,
                        &ctx,
                        IdempotencyScope::Reindex,
                        &key,
                        &request_hash,
                        api_idempotency::DEFAULT_IN_PROGRESS_TTL,
                    )
                    .await?
                    {
                        api_idempotency::IdempotencyClaim::Replay(stored) => {
                            serde_json::from_value(stored.response_body).map_err(|_| {
                                crate::db::error::DbError::Config("idempotency_corrupt".into())
                            })
                        }
                        api_idempotency::IdempotencyClaim::Proceed => {
                            let outcome = indexing::enqueue_document_reindex_within_txn(
                                txn,
                                &ctx,
                                document_id,
                            )
                            .await
                            .map_err(catalog_to_db)?;
                            let body = ReindexResponse {
                                document_id: outcome.document_id,
                                version_id: outcome.version_id,
                                job_id: outcome.job.id,
                                created: outcome.created,
                                request_id: request_id.clone(),
                            };
                            let response_body = serde_json::to_value(&body).map_err(|_| {
                                crate::db::error::DbError::Config("idempotency_corrupt".into())
                            })?;
                            api_idempotency::finalize(
                                txn,
                                &ctx,
                                IdempotencyScope::Reindex,
                                &key,
                                &request_hash,
                                200,
                                &response_body,
                            )
                            .await?;
                            Ok(body)
                        }
                    }
                })
            }
        })
        .await
        .map_err(|error| map_reindex_txn(error, &request_id))?;
        return Ok(Json(body));
    }

    let outcome = indexing::enqueue_document_reindex(state.pool(), &auth.context, document_id)
        .await
        .map_err(|error| map_catalog_index(error, &request_id))?;
    Ok(Json(ReindexResponse {
        document_id: outcome.document_id,
        version_id: outcome.version_id,
        job_id: outcome.job.id,
        created: outcome.created,
        request_id,
    }))
}

async fn list_versions(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(document_id): AppPath<Uuid>,
    AppQuery(query): AppQuery<VersionListQuery>,
) -> Result<Json<ListResponse<DocumentVersionResponse>>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let page = PageParams::from_query(query.limit, query.cursor, &request_id)?;
    let _ = load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    let after = match page.cursor.as_deref() {
        Some(raw) => Some(
            decode_cursor::<VersionNumberIdCursor>(raw).map_err(|message| {
                ApiRejection::validation(message, &request_id)
                    .with_details(serde_json::json!({ "field": "cursor" }))
            })?,
        ),
        None => None,
    };
    let fetch_limit = i64::from(page.limit) + 1;
    let mut rows = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let after_version_number = after.as_ref().map(|cursor| cursor.version_number);
        let after_id = after.as_ref().map(|cursor| cursor.id);
        move |txn| {
            Box::pin(async move {
                document_versions::list_page_by_document(
                    txn,
                    &ctx,
                    document_id,
                    fetch_limit,
                    after_version_number,
                    after_id,
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| map_db(error, &request_id))?;
    let has_more = rows.len() as u32 > page.limit;
    if has_more {
        rows.truncate(page.limit as usize);
    }
    let next_cursor = if has_more {
        rows.last().and_then(|row| {
            encode_cursor(&VersionNumberIdCursor {
                version_number: row.version_number,
                id: row.id,
            })
            .ok()
        })
    } else {
        None
    };
    let page_info = match next_cursor {
        Some(cursor) => PageInfo::more(cursor),
        None => PageInfo::end(),
    };
    Ok(Json(ListResponse {
        items: rows
            .into_iter()
            .map(|row| version_response(row, request_id.clone()))
            .collect(),
        page_info,
        request_id,
    }))
}

async fn get_version(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath((document_id, version_id)): AppPath<(Uuid, Uuid)>,
) -> Result<Json<DocumentVersionResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let _ = load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    let version = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                document_versions::find_by_id(txn, &ctx, document_id, version_id).await
            })
        }
    })
    .await
    .map_err(|error| map_db(error, &request_id))?
    .ok_or_else(|| deny_or_not_found(&request_id))?;
    Ok(Json(version_response(version, request_id)))
}

async fn diff_versions(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath((document_id, left_version_id, right_version_id)): AppPath<(Uuid, Uuid, Uuid)>,
) -> Result<Json<VersionDiffResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let _ = load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    let (left, right) = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                let left =
                    document_versions::find_by_id(txn, &ctx, document_id, left_version_id).await?;
                let right =
                    document_versions::find_by_id(txn, &ctx, document_id, right_version_id).await?;
                Ok::<_, crate::db::error::DbError>((left, right))
            })
        }
    })
    .await
    .map_err(|error| map_db(error, &request_id))?;
    let left = left.ok_or_else(|| deny_or_not_found(&request_id))?;
    let right = right.ok_or_else(|| deny_or_not_found(&request_id))?;
    Ok(Json(VersionDiffResponse {
        document_id,
        left_version_id: left.id,
        right_version_id: right.id,
        left_version_number: left.version_number,
        right_version_number: right.version_number,
        content_sha256_changed: left.content_sha256 != right.content_sha256,
        publication_state_changed: left.publication_state != right.publication_state,
        current_flag_changed: left.is_current != right.is_current,
        change_summary_changed: left.change_summary != right.change_summary,
        request_id,
    }))
}

async fn publish_version(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath((document_id, version_id)): AppPath<(Uuid, Uuid)>,
) -> Result<Json<ReindexResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, "doc.publish", &request_id)?;
    let _ = load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    let outcome =
        indexing::publish_document_version(state.pool(), &auth.context, document_id, version_id)
            .await
            .map_err(|error| map_catalog_index(error, &request_id))?;
    Ok(Json(ReindexResponse {
        document_id: outcome.document_id,
        version_id: outcome.version_id,
        job_id: outcome.job.id,
        created: outcome.created,
        request_id,
    }))
}

async fn list_conflicts(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppQuery(query): AppQuery<ConflictListQuery>,
) -> Result<Json<ListResponse<ConflictResponse>>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, "qa.query", &request_id)?;
    let page = PageParams::from_query(query.limit, query.cursor, &request_id)?;
    let status = match query.status.as_deref() {
        None => None,
        Some(raw) => Some(
            ConflictStatus::parse(raw)
                .map_err(|message| ApiRejection::validation(message, &request_id))?,
        ),
    };
    let after = match page.cursor.as_deref() {
        Some(raw) => Some(decode_cursor::<CreatedAtIdCursor>(raw).map_err(|message| {
            ApiRejection::validation(message, &request_id)
                .with_details(serde_json::json!({ "field": "cursor" }))
        })?),
        None => None,
    };
    let allowed: Vec<Uuid> = auth
        .context
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect();
    let fetch_limit = i64::from(page.limit) + 1;
    let mut rows = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let after_detected_at = after.as_ref().map(|cursor| cursor.created_at);
        let after_id = after.as_ref().map(|cursor| cursor.id);
        move |txn| {
            Box::pin(async move {
                conflicts::list_authorized_page(
                    txn,
                    &ctx,
                    &allowed,
                    status,
                    fetch_limit,
                    after_detected_at,
                    after_id,
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| map_db(error, &request_id))?;
    let has_more = rows.len() as u32 > page.limit;
    if has_more {
        rows.truncate(page.limit as usize);
    }
    let next_cursor = if has_more {
        rows.last().and_then(|row| {
            encode_cursor(&CreatedAtIdCursor {
                created_at: row.first_detected_at,
                id: row.id,
            })
            .ok()
        })
    } else {
        None
    };
    let page_info = match next_cursor {
        Some(cursor) => PageInfo::more(cursor),
        None => PageInfo::end(),
    };
    Ok(Json(ListResponse {
        items: rows
            .into_iter()
            .map(|row| conflict_response(row, request_id.clone()))
            .collect(),
        page_info,
        request_id,
    }))
}

async fn get_conflict(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(conflict_id): AppPath<Uuid>,
) -> Result<Json<ConflictResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, "qa.query", &request_id)?;
    let allowed: Vec<Uuid> = auth
        .context
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect();
    let conflict = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(
                async move { conflicts::get_authorized(txn, &ctx, &allowed, conflict_id).await },
            )
        }
    })
    .await
    .map_err(|error| map_db(error, &request_id))?;
    Ok(Json(conflict_response(conflict, request_id)))
}

async fn triage_conflict(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(conflict_id): AppPath<Uuid>,
    AppJson(body): AppJson<TriageBody>,
) -> Result<Json<ConflictResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, "doc.publish", &request_id)?;
    let status = ConflictStatus::parse(&body.status)
        .map_err(|message| ApiRejection::validation(message, &request_id))?;
    if status == ConflictStatus::Open {
        return Err(ApiRejection::validation(
            "triage status must be terminal",
            &request_id,
        ));
    }
    if let Some(ref note) = body.resolution_note {
        if note.len() > 2000 {
            return Err(ApiRejection::validation(
                "resolutionNote must be at most 2000 characters",
                &request_id,
            ));
        }
    }
    let allowed: Vec<Uuid> = auth
        .context
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect();
    let note = body.resolution_note.clone();
    let conflict = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                conflicts::triage(
                    txn,
                    &ctx,
                    &allowed,
                    TriageConflict {
                        conflict_id,
                        status,
                        resolution_note: note.as_deref(),
                        resolution_version_a_id: body.resolution_version_a_id,
                        resolution_version_b_id: body.resolution_version_b_id,
                    },
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| map_db(error, &request_id))?;
    Ok(Json(conflict_response(conflict, request_id)))
}

async fn list_conflict_evidence(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(conflict_id): AppPath<Uuid>,
    AppQuery(query): AppQuery<EvidenceListQuery>,
) -> Result<Json<ListResponse<ConflictEvidenceResponse>>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, "qa.query", &request_id)?;
    let page = PageParams::from_query(query.limit, query.cursor, &request_id)?;
    let after = match page.cursor.as_deref() {
        Some(raw) => Some(decode_cursor::<CreatedAtIdCursor>(raw).map_err(|message| {
            ApiRejection::validation(message, &request_id)
                .with_details(serde_json::json!({ "field": "cursor" }))
        })?),
        None => None,
    };
    let allowed: Vec<Uuid> = auth
        .context
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect();
    let fetch_limit = i64::from(page.limit) + 1;
    let mut evidence = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let after_created_at = after.as_ref().map(|cursor| cursor.created_at);
        let after_id = after.as_ref().map(|cursor| cursor.id);
        move |txn| {
            Box::pin(async move {
                conflicts::list_authorized_evidence_page(
                    txn,
                    &ctx,
                    &allowed,
                    conflict_id,
                    fetch_limit,
                    after_created_at,
                    after_id,
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| map_db(error, &request_id))?;
    let has_more = evidence.len() as u32 > page.limit;
    if has_more {
        evidence.truncate(page.limit as usize);
    }
    let next_cursor = if has_more {
        evidence.last().and_then(|row| {
            encode_cursor(&CreatedAtIdCursor {
                created_at: row.created_at,
                id: row.id,
            })
            .ok()
        })
    } else {
        None
    };
    let page_info = match next_cursor {
        Some(cursor) => PageInfo::more(cursor),
        None => PageInfo::end(),
    };
    Ok(Json(ListResponse {
        items: evidence
            .into_iter()
            .map(|row| ConflictEvidenceResponse {
                id: row.id,
                conflict_id: row.conflict_id,
                claim_id: row.claim_id,
                evidence_role: row.evidence_role.as_str().into(),
                citation_quote: row.citation_quote,
                created_at: row.created_at,
            })
            .collect(),
        page_info,
        request_id,
    }))
}

fn map_catalog_index(error: CatalogIndexError, request_id: &str) -> ApiRejection {
    match error {
        CatalogIndexError::NotFound => deny_or_not_found(request_id),
        CatalogIndexError::NoCurrentVersion => ApiRejection::conflict(
            "no_current_version",
            "Document has no current version",
            request_id,
        ),
        CatalogIndexError::GenerationMissing => ApiRejection::conflict(
            "index_generation_missing",
            "No active index generation for collection",
            request_id,
        ),
        CatalogIndexError::VersionSuperseded => ApiRejection::conflict(
            "version_superseded",
            "Version is already published and not current",
            request_id,
        ),
        CatalogIndexError::InvalidPublish => {
            ApiRejection::validation("Publish request is invalid", request_id)
        }
        CatalogIndexError::StateConflict => {
            ApiRejection::conflict("conflict_state", "Resource state conflict", request_id)
        }
        CatalogIndexError::Job | CatalogIndexError::Database => ApiRejection::internal(request_id),
    }
}

fn catalog_to_db(error: CatalogIndexError) -> crate::db::error::DbError {
    match error {
        CatalogIndexError::NotFound => crate::db::error::DbError::NotFound,
        CatalogIndexError::NoCurrentVersion => {
            crate::db::error::DbError::Config("no_current_version".into())
        }
        CatalogIndexError::GenerationMissing => {
            crate::db::error::DbError::Config("index_generation_missing".into())
        }
        CatalogIndexError::VersionSuperseded => {
            crate::db::error::DbError::Config("version_superseded".into())
        }
        CatalogIndexError::InvalidPublish => {
            crate::db::error::DbError::Config("invalid_publish".into())
        }
        CatalogIndexError::StateConflict => crate::db::error::DbError::StaleState {
            expected: "indexable".into(),
            observed: "conflict".into(),
        },
        CatalogIndexError::Job | CatalogIndexError::Database => {
            crate::db::error::DbError::Config("catalog_index_failed".into())
        }
    }
}

fn map_reindex_txn(error: crate::db::error::DbError, request_id: &str) -> ApiRejection {
    match &error {
        crate::db::error::DbError::Config(message) if message == "no_current_version" => {
            ApiRejection::conflict(
                "no_current_version",
                "Document has no current version",
                request_id,
            )
        }
        crate::db::error::DbError::Config(message) if message == "index_generation_missing" => {
            ApiRejection::conflict(
                "index_generation_missing",
                "No active index generation for collection",
                request_id,
            )
        }
        _ => map_db(error, request_id),
    }
}

fn conflict_response(
    conflict: crate::db::models::Conflict,
    request_id: String,
) -> ConflictResponse {
    ConflictResponse {
        id: conflict.id,
        status: conflict.status.as_str().into(),
        severity: conflict.severity.as_str().into(),
        conflict_type: conflict.conflict_type.as_str().into(),
        claim_a_id: conflict.claim_a_id,
        claim_b_id: conflict.claim_b_id,
        first_detected_at: conflict.first_detected_at,
        first_detected_version_id: conflict.first_detected_version_id,
        resolved_at: conflict.resolved_at,
        resolution_note: conflict.resolution_note,
        resolution_version_a_id: conflict.resolution_version_a_id,
        resolution_version_b_id: conflict.resolution_version_b_id,
        created_at: conflict.created_at,
        updated_at: conflict.updated_at,
        request_id,
    }
}

fn page_info_from_created(rows: &mut Vec<crate::db::models::Document>, limit: u32) -> PageInfo {
    let has_more = rows.len() as u32 > limit;
    if has_more {
        rows.truncate(limit as usize);
    }
    if !has_more {
        return PageInfo::end();
    }
    let Some(last) = rows.last() else {
        return PageInfo::end();
    };
    match encode_cursor(&CreatedAtIdCursor {
        created_at: last.created_at,
        id: last.id,
    }) {
        Ok(cursor) => PageInfo::more(cursor),
        Err(_) => PageInfo::end(),
    }
}

fn map_deletion(error: DeletionError, request_id: &str) -> ApiRejection {
    match error {
        DeletionError::Db(error) => map_db(error, request_id),
        DeletionError::UnexpectedState(_) => ApiRejection::conflict(
            "document_state_invalid",
            "Document cannot be deleted in its current state",
            request_id,
        ),
        _ => ApiRejection::internal(request_id),
    }
}

enum DocumentRouteError {
    Citation(CitationError, String),
    Preview(PreviewError, String),
    Download(DownloadError, String),
    Validation(String, String),
}

impl DocumentRouteError {
    fn validation(message: impl Into<String>, request_id: String) -> Self {
        Self::Validation(message.into(), request_id)
    }
}

fn download_status(error: &DownloadError) -> StatusCode {
    match error {
        DownloadError::PermissionDenied => StatusCode::FORBIDDEN,
        DownloadError::NotFound => StatusCode::NOT_FOUND,
        DownloadError::Expired | DownloadError::Replay | DownloadError::InvalidToken => {
            StatusCode::GONE
        }
        DownloadError::SignerNotConfigured
        | DownloadError::StorageUnavailable
        | DownloadError::Busy => StatusCode::SERVICE_UNAVAILABLE,
        DownloadError::InvalidPurpose | DownloadError::InvalidTtl => StatusCode::BAD_REQUEST,
        DownloadError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        DownloadError::Integrity => StatusCode::CONFLICT,
        DownloadError::Storage | DownloadError::Database => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for DocumentRouteError {
    fn into_response(self) -> Response {
        let (status, code, message, request_id) = match self {
            Self::Citation(error, request_id) => {
                let status = match error {
                    CitationError::PermissionDenied => StatusCode::FORBIDDEN,
                    CitationError::NotFound | CitationError::MarkdownMissing => {
                        StatusCode::NOT_FOUND
                    }
                    CitationError::InvalidRequest => StatusCode::BAD_REQUEST,
                    CitationError::QuoteMismatch
                    | CitationError::VersionMismatch
                    | CitationError::InvalidAnchor
                    | CitationError::Integrity => StatusCode::CONFLICT,
                    CitationError::Storage | CitationError::Database => {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                };
                (status, error.code(), error.to_string(), request_id)
            }
            Self::Preview(error, request_id) => {
                let status = match error {
                    PreviewError::PermissionDenied => StatusCode::FORBIDDEN,
                    PreviewError::NotFound | PreviewError::MarkdownMissing => StatusCode::NOT_FOUND,
                    PreviewError::StorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
                    PreviewError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
                    PreviewError::Integrity | PreviewError::InvalidUtf8 => StatusCode::CONFLICT,
                    PreviewError::Storage | PreviewError::Database => {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                };
                (status, error.code(), error.to_string(), request_id)
            }
            Self::Download(error, request_id) => (
                download_status(&error),
                error.code(),
                error.to_string(),
                request_id,
            ),
            Self::Validation(message, request_id) => (
                StatusCode::BAD_REQUEST,
                "validation_failed",
                message,
                request_id,
            ),
        };
        (
            status,
            Json(ApiError {
                code: code.into(),
                message,
                request_id,
                details: None,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[test]
    fn redeem_body_debug_redacts_token() {
        let body = RedeemDownloadBody {
            token: RedactedToken("mhdl1.aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa.deadbeef".into()),
        };
        let debug = format!("{body:?}");
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("deadbeef"));
        assert!(!debug.contains("mhdl1"));
    }

    #[test]
    fn mint_response_debug_redacts_token() {
        let response = MintDownloadResponse {
            capability_id: Uuid::nil(),
            token: RedactedToken("mhdl1.secret-token-value".into()),
            purpose: "markdown".into(),
            document_id: Uuid::nil(),
            version_id: Uuid::nil(),
            expires_at: chrono::Utc::now(),
            request_id: "r".into(),
        };
        let debug = format!("{response:?}");
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("secret-token-value"));
    }

    #[tokio::test]
    async fn download_error_mapping_distinguishes_expired_replay_busy() {
        for (error, status, code) in [
            (DownloadError::Expired, StatusCode::GONE, "download_expired"),
            (DownloadError::Replay, StatusCode::GONE, "download_replay"),
            (
                DownloadError::Busy,
                StatusCode::SERVICE_UNAVAILABLE,
                "download_busy",
            ),
            (
                DownloadError::Integrity,
                StatusCode::CONFLICT,
                "download_integrity",
            ),
            (
                DownloadError::TooLarge,
                StatusCode::PAYLOAD_TOO_LARGE,
                "download_too_large",
            ),
        ] {
            let response = DocumentRouteError::Download(error, "req-1".into()).into_response();
            assert_eq!(response.status(), status);
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["code"], code);
            assert_eq!(json["requestId"], "req-1");
        }
    }
}
