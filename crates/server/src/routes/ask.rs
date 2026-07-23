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
use crate::db::ask_streams;
use crate::db::models::AuditOutcome;
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::services::audit;
use crate::services::qa::ask_stream::{self, AskStreamPrepareError};
use crate::services::qa::{ask, AskRequest};
use crate::services::retrieval::{RetrievalError, VersionMode};
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
            body.collection_ids
                .map(|ids| ids.into_iter().collect::<BTreeSet<_>>()),
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
            AskStreamPrepareError::Database => RouteError::Database(auth.request_id.clone()),
        })?;
        let session_id_str = started.session_id.to_string();
        let _ = audit::record(
            state.pool(),
            &auth.context,
            audit::AuditRecord {
                request_id: &auth.request_id,
                action: "ask.stream",
                resource_type: "ask_stream",
                resource_id: Some(&session_id_str),
                outcome: AuditOutcome::Success,
                metadata: serde_json::json!({
                    "streamSessionId": started.session_id,
                }),
            },
        )
        .await;
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
    if body.question.trim().is_empty() || body.question.len() > 8_192 {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid question",
        ));
    }
    let mode = parse_mode(&body)
        .map_err(|message| RouteError::Validation(auth.request_id.clone(), message))?;
    let vector_index = state
        .vector_index()
        .ok_or_else(|| RouteError::Unavailable(auth.request_id.clone()))?;
    let response = ask(
        state.pool(),
        vector_index,
        state.embedder(),
        state.chat_provider(),
        &auth.context,
        AskRequest {
            question: body.question,
            collection_ids: body
                .collection_ids
                .map(|ids| ids.into_iter().collect::<BTreeSet<_>>()),
            mode,
            limit: body.limit.clamp(1, 20),
            conflict_ids: body.conflict_ids,
        },
    )
    .await
    .map_err(|error| match error {
        crate::services::qa::AskError::Retrieval(error) => {
            RouteError::from_retrieval(error, &auth.request_id)
        }
        crate::services::qa::AskError::InvalidRequest(message) => {
            RouteError::Validation(auth.request_id.clone(), message)
        }
        crate::services::qa::AskError::Provider(_) => {
            RouteError::Unavailable(auth.request_id.clone())
        }
    })?;
    let _ = audit::record(
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
                "citationCount": response.citations.len(),
            }),
        },
    )
    .await;
    Ok(response)
}

enum RouteError {
    Validation(String, &'static str),
    Denied(String),
    NotFound(String),
    Unavailable(String),
    Database(String),
    StreamClosed(String, &'static str),
    RateLimited(crate::routes::rate_limit_guard::RateLimitRejected),
}

impl RouteError {
    fn from_retrieval(error: RetrievalError, request_id: &str) -> Self {
        match error {
            RetrievalError::PermissionDenied | RetrievalError::EmptyScope => {
                Self::Denied(request_id.to_string())
            }
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
