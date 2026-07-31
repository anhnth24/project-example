//! Grounded ask + durable resumable SSE stream routes (P1B-R03/R05).

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures::stream;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::{resolve_last_event_id, ApiError, LastEventIdError};
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::ask_streams;
use crate::db::error::DbError;
use crate::db::models::AuditOutcome;
use crate::db::pool::with_org_txn;
use crate::db::projects;
use crate::http::AppState;
use crate::services::audit;
use crate::services::qa::ask_stream::{self, AskStreamPrepareError};
use crate::services::qa::{ask, AskRequest};
use crate::services::retrieval::{RetrievalError, VersionMode, PERMISSION_QA_QUERY};
use crate::services::stream_auth;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/ask", post(ask_json))
        .route("/api/v1/ask/stream", post(ask_stream_route))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskBody {
    question: String,
    #[serde(default)]
    collection_ids: Option<Vec<Uuid>>,
    /// P2-18 — see `routes::search::SearchBody::project_id`'s doc; identical
    /// contract, shared resolver (`db::projects::resolve_project_scope`).
    /// Deprecated (P2-19): kept working as-is; prefer `project_ids` below.
    #[serde(default)]
    project_id: Option<Uuid>,
    /// P2-19 — see `routes::search::SearchBody::project_ids`'s doc; identical
    /// contract (union, bounded, 404 on any unknown id, unions with
    /// `projectId` when both are given), shared merge/resolve helpers.
    #[serde(default)]
    project_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    as_of: Option<DateTime<Utc>>,
    #[serde(default)]
    document_id: Option<Uuid>,
    #[serde(default)]
    version_a: Option<Uuid>,
    #[serde(default)]
    version_b: Option<Uuid>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    conflict_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    #[serde(rename = "lastEventId")]
    last_event_id: Option<String>,
    #[serde(rename = "streamSessionId")]
    stream_session_id: Option<Uuid>,
}

fn default_limit() -> usize {
    8
}

fn parse_mode(body: &AskBody) -> Result<VersionMode, &'static str> {
    match body.mode.as_deref().unwrap_or("current") {
        "current" => Ok(VersionMode::Current),
        "as_of" => Ok(VersionMode::AsOf {
            at: body.as_of.ok_or("as_of requires asOf timestamp")?,
        }),
        "compare" => Ok(VersionMode::Compare {
            document_id: body.document_id.ok_or("compare requires documentId")?,
            version_a: body.version_a.ok_or("compare requires versionA")?,
            version_b: body.version_b.ok_or("compare requires versionB")?,
        }),
        "history" => Ok(VersionMode::History {
            document_id: body.document_id.ok_or("history requires documentId")?,
        }),
        _ => Err("mode must be current|as_of|compare|history"),
    }
}

async fn ask_json(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    client_ip: Option<axum::Extension<crate::middleware::ClientIp>>,
    Json(body): Json<AskBody>,
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
    crate::routes::rate_limit_guard::check_route(&state, "ask", &ip, &auth.request_id)
        .map_err(RouteError::RateLimited)?;
    let response = run_ask(&state, &auth, body).await?;
    Ok(Json(serde_json::json!({
        "answer": response.answer,
        "mode": response.mode.as_str(),
        "citations": response.citations,
        "warnings": response.warnings,
        "versionContext": response.version_context,
        "embeddingMode": response.embedding_mode,
        "requestId": auth.request_id,
    })))
}

async fn ask_stream_route(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    client_ip: Option<axum::Extension<crate::middleware::ClientIp>>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
    Json(body): Json<AskBody>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>> + Send>, RouteError> {
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
    crate::routes::rate_limit_guard::check_route(&state, "ask", &ip, &auth.request_id)
        .map_err(RouteError::RateLimited)?;

    let header = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok());

    // Parse syntax/conflict before any session/provider side effects.
    let cursor_syntax = resolve_last_event_id(query.last_event_id.as_deref(), header, None)
        .map_err(|error| RouteError::Validation(auth.request_id.clone(), error.message()))?;

    let (session_id, cited_document_ids, cancel, last_event_id) = if let Some(session_id) =
        query.stream_session_id
    {
        // Resume against pinned session — never re-run retrieval/provider.
        let session = with_org_txn(state.pool(), &auth.context, {
            let ctx = auth.context.clone();
            move |txn| {
                Box::pin(async move { ask_streams::get_owned_session(txn, &ctx, session_id).await })
            }
        })
        .await
        .map_err(|error| match error {
            crate::db::error::DbError::NotFound => RouteError::NotFound(auth.request_id.clone()),
            _ => RouteError::Database(auth.request_id.clone()),
        })?;
        let high_water = session.high_water_sequence();
        let last_event_id =
            resolve_last_event_id(query.last_event_id.as_deref(), header, Some(high_water))
                .map_err(|error| {
                    RouteError::Validation(auth.request_id.clone(), error.message())
                })?;
        stream_auth::revalidate_ask_stream(state.pool(), &auth.claims, &session.cited_document_ids)
            .await
            .map_err(|error| {
                RouteError::StreamClosed(auth.request_id.clone(), error.close_reason())
            })?;
        (session.id, session.cited_document_ids, None, last_event_id)
    } else {
        // Fresh streams only accept cursor 0 (side-effect free on invalid).
        if cursor_syntax != 0 {
            return Err(RouteError::Validation(
                auth.request_id.clone(),
                LastEventIdError::OutOfRange.message(),
            ));
        }
        let mode = parse_mode(&body)
            .map_err(|message| RouteError::Validation(auth.request_id.clone(), message))?;
        // P2-19 — merge/bound before any database round trip.
        let project_ids = projects::merge_project_ids(body.project_id, body.project_ids)
            .map_err(|message| RouteError::Validation(auth.request_id.clone(), message))?;
        let question_chars = body.question.len();
        let collection_ids = body
            .collection_ids
            .map(|ids| ids.into_iter().collect::<BTreeSet<_>>());
        // P2-18/P2-19 — same narrow-never-widen union project scope as
        // routes::search / ask_json's run_ask above. Resolved before the
        // `vector_index` availability check right below — see that check's
        // own comment in routes::search for why.
        let collection_ids = projects::resolve_project_scope(
            state.pool(),
            &auth.context,
            &project_ids,
            collection_ids,
        )
        .await
        .map_err(|error| match error {
            DbError::NotFound => RouteError::ProjectNotFound(auth.request_id.clone()),
            _ => RouteError::Database(auth.request_id.clone()),
        })?;
        let vector_index = state
            .vector_index()
            .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
        let started = ask_stream::start_ask_stream(
            state.pool(),
            vector_index,
            state.embedder(),
            state.chat_provider().cloned(),
            &auth.context,
            auth.claims.clone(),
            auth.request_id.clone(),
            body.question,
            collection_ids,
            mode,
            body.limit.clamp(1, 20),
            body.conflict_ids,
        )
        .await
        .map_err(|error| match error {
            AskStreamPrepareError::InvalidRequest(message) => {
                RouteError::Validation(auth.request_id.clone(), message)
            }
            AskStreamPrepareError::Retrieval(error) => {
                RouteError::from_retrieval(error, &auth.request_id)
            }
            AskStreamPrepareError::Quota(error) => {
                RouteError::Quota(error, auth.request_id.clone())
            }
            AskStreamPrepareError::Database => RouteError::Database(auth.request_id.clone()),
        });
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                if let RouteError::Quota(quota_error, _) = &error {
                    audit_token_quota_deny(&state, &auth, quota_error).await?;
                }
                return Err(error);
            }
        };
        let session_id_str = started.session_id.to_string();
        audit::record(
            state.pool(),
            &auth.context,
            audit::AuditRecord {
                request_id: &auth.request_id,
                action: "ask.stream",
                resource_type: "ask_stream",
                resource_id: Some(&session_id_str),
                outcome: AuditOutcome::Success,
                metadata: serde_json::json!({
                    "stream_session_id": started.session_id.to_string(),
                    "question_chars": question_chars,
                }),
            },
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        (
            started.session_id,
            started.cited_document_ids,
            Some(started.cancel),
            0,
        )
    };

    let rx = ask_stream::live_tail_ask_session(
        state.pool().clone(),
        auth.claims.clone(),
        session_id,
        auth.request_id.clone(),
        cited_document_ids,
        last_event_id,
        cancel,
    )
    .await;

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    });
    Ok(Sse::new(stream).keep_alive(ask_stream::keep_alive()))
}

async fn run_ask(
    state: &AppState,
    auth: &AuthenticatedOrg,
    body: AskBody,
) -> Result<crate::services::qa::AskResponse, RouteError> {
    if require_permission(&auth.context, PERMISSION_QA_QUERY).is_err() {
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "ask.query",
            "ask",
            None,
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    if body.question.trim().is_empty() || body.question.len() > 8_192 {
        audit::record(
            state.pool(),
            &auth.context,
            audit::AuditRecord {
                request_id: &auth.request_id,
                action: "ask.query",
                resource_type: "ask",
                resource_id: None,
                outcome: AuditOutcome::Error,
                metadata: serde_json::json!({ "reason": "validation_failed" }),
            },
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid question",
        ));
    }
    let mode = parse_mode(&body)
        .map_err(|message| RouteError::Validation(auth.request_id.clone(), message))?;
    // P2-19 — merge/bound before any database round trip.
    let project_ids = projects::merge_project_ids(body.project_id, body.project_ids)
        .map_err(|message| RouteError::Validation(auth.request_id.clone(), message))?;
    let question_chars = body.question.len();
    let collection_ids = body
        .collection_ids
        .map(|ids| ids.into_iter().collect::<BTreeSet<_>>());
    // P2-18/P2-19 — same narrow-never-widen union project scope as
    // routes::search. Resolved before the `vector_index` availability check
    // right below — see that check's own comment in routes::search for why.
    let collection_ids =
        projects::resolve_project_scope(state.pool(), &auth.context, &project_ids, collection_ids)
            .await
            .map_err(|error| match error {
                DbError::NotFound => RouteError::ProjectNotFound(auth.request_id.clone()),
                _ => RouteError::Database(auth.request_id.clone()),
            })?;
    let vector_index = state
        .vector_index()
        .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
    let response = match ask(
        state.pool(),
        vector_index,
        state.embedder(),
        state.chat_provider(),
        &auth.context,
        AskRequest {
            question: body.question,
            collection_ids,
            mode,
            limit: body.limit.clamp(1, 20),
            conflict_ids: body.conflict_ids,
        },
    )
    .await
    {
        Ok(response) => response,
        Err(crate::services::qa::AskError::Retrieval(
            RetrievalError::PermissionDenied | RetrievalError::EmptyScope,
        )) => {
            audit::record_deny(
                state.pool(),
                &auth.context,
                &auth.request_id,
                "ask.query",
                "ask",
                None,
                "permission_denied",
            )
            .await
            .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
            return Err(RouteError::Denied(auth.request_id.clone()));
        }
        Err(crate::services::qa::AskError::InvalidRequest(message)) => {
            audit::record(
                state.pool(),
                &auth.context,
                audit::AuditRecord {
                    request_id: &auth.request_id,
                    action: "ask.query",
                    resource_type: "ask",
                    resource_id: None,
                    outcome: AuditOutcome::Error,
                    metadata: serde_json::json!({ "reason": "validation_failed" }),
                },
            )
            .await
            .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
            return Err(RouteError::Validation(auth.request_id.clone(), message));
        }
        Err(crate::services::qa::AskError::Provider(_)) => {
            audit::record(
                state.pool(),
                &auth.context,
                audit::AuditRecord {
                    request_id: &auth.request_id,
                    action: "ask.query",
                    resource_type: "ask",
                    resource_id: None,
                    outcome: AuditOutcome::Error,
                    metadata: serde_json::json!({ "reason": "provider_error" }),
                },
            )
            .await
            .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
            return Err(RouteError::Unavailable(auth.request_id.clone()));
        }
        Err(crate::services::qa::AskError::Retrieval(error)) => {
            return Err(RouteError::from_retrieval(error, &auth.request_id));
        }
        Err(crate::services::qa::AskError::Quota(error)) => {
            audit_token_quota_deny(state, auth, &error).await?;
            return Err(RouteError::Quota(error, auth.request_id.clone()));
        }
    };
    audit::record(
        state.pool(),
        &auth.context,
        audit::AuditRecord {
            request_id: &auth.request_id,
            action: "ask.query",
            resource_type: "ask",
            resource_id: None,
            outcome: AuditOutcome::Success,
            metadata: serde_json::json!({
                "mode": response.mode.as_str(),
                "citation_count": response.citations.len(),
                "question_chars": question_chars,
            }),
        },
    )
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    Ok(response)
}

/// Durable `quota.deny` audit for a token-quota denial on ask (1C-09 a).
/// Only the exceeded case is a deny; infrastructure quota errors surface as
/// plain 5xx without a deny row.
async fn audit_token_quota_deny(
    state: &AppState,
    auth: &AuthenticatedOrg,
    error: &crate::services::quota::QuotaError,
) -> Result<(), RouteError> {
    if !matches!(error, crate::services::quota::QuotaError::QuotaExceeded(_)) {
        return Ok(());
    }
    audit::record(
        state.pool(),
        &auth.context,
        audit::AuditRecord {
            request_id: &auth.request_id,
            action: "quota.deny",
            resource_type: "quota",
            resource_id: Some("tokens"),
            outcome: AuditOutcome::Deny,
            metadata: serde_json::json!({
                "reason": "quota_exceeded",
                "resource_kind": "tokens",
            }),
        },
    )
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))
}

enum RouteError {
    Validation(String, &'static str),
    Denied(String),
    NotFound(String),
    /// P2-18 — distinct from `NotFound` (stream session) purely so the
    /// error message stays accurate; same status/code.
    ProjectNotFound(String),
    Unavailable(String),
    Database(String),
    StreamClosed(String, &'static str),
    RateLimited(crate::routes::rate_limit_guard::RateLimitRejected),
    /// Token-quota admission (1C-09 a): rendered through the shared
    /// `QuotaError` HTTP contract (429 `quota_exceeded` + x-quota-* headers,
    /// same as storage quota on upload).
    Quota(crate::services::quota::QuotaError, String),
}

impl RouteError {
    fn from_retrieval(error: RetrievalError, request_id: &str) -> Self {
        match error {
            RetrievalError::PermissionDenied => Self::NotFound(request_id.to_string()),
            RetrievalError::EmptyScope => Self::Denied(request_id.to_string()),
            RetrievalError::InvalidRequest(_) | RetrievalError::LineageMismatch => {
                Self::Validation(request_id.to_string(), "Invalid ask request")
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
        if let Self::Quota(error, request_id) = self {
            return error.into_response_with_request_id(&request_id);
        }
        let (status, code, message, request_id) = match self {
            Self::Validation(request_id, message) => (
                StatusCode::BAD_REQUEST,
                "validation_failed",
                message,
                request_id,
            ),
            Self::Denied(request_id) => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "Permission denied",
                request_id,
            ),
            Self::NotFound(request_id) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Stream session not found",
                request_id,
            ),
            Self::ProjectNotFound(request_id) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Project not found",
                request_id,
            ),
            Self::Unavailable(request_id) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "dependency_unavailable",
                "Ask dependencies unavailable",
                request_id,
            ),
            Self::Database(request_id) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Ask failed",
                request_id,
            ),
            Self::StreamClosed(request_id, reason) => (
                StatusCode::UNAUTHORIZED,
                reason,
                "Stream authorization closed",
                request_id,
            ),
            Self::RateLimited(_) | Self::Quota(..) => unreachable!(),
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
