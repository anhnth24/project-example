//! Grounded ask + SSE stream routes (P1B-R03/R05).

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures::stream;
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::middleware::AuthenticatedOrg;
use crate::db::models::AuditOutcome;
use crate::http::AppState;
use crate::services::audit;
use crate::services::qa::stream::{
    ask_response_events, auth_closed_envelope, into_sse_stream, replay_from, MAX_BUFFERED_EVENTS,
};
use crate::services::qa::{ask, AskRequest};
use crate::services::retrieval::{RetrievalError, VersionMode};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/ask", post(ask_json))
        .route("/api/v1/ask/stream", post(ask_stream))
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
    last_event_id: Option<u64>,
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

async fn ask_stream(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    client_ip: Option<axum::Extension<crate::middleware::ClientIp>>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
    Json(body): Json<AskBody>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, RouteError> {
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
    let last_event_id = query.last_event_id.or_else(|| {
        headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
    });
    let request_id = auth.request_id.clone();
    let claims = auth.claims.clone();
    let response = run_ask(&state, &auth, body).await?;
    let cited_document_ids: Vec<Uuid> = response
        .citations
        .iter()
        .map(|pin| pin.logical_document_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut events = ask_response_events(&request_id, &response);
    events = replay_from(&events, last_event_id);

    // Bound slow clients; revalidate principal + cited docs each batch.
    let (tx, rx) = mpsc::channel::<Event>(MAX_BUFFERED_EVENTS.min(64));
    let pool = state.pool().clone();
    const BATCH: usize = 8;
    const SEND_TIMEOUT: Duration = Duration::from_secs(5);
    tokio::spawn(async move {
        let mut seq_hint = 0_u64;
        for chunk in events.chunks(BATCH) {
            match crate::services::stream_auth::revalidate_ask_stream(
                &pool,
                &claims,
                &cited_document_ids,
            )
            .await
            {
                Ok(_) => {}
                Err(error) => {
                    let closed = auth_closed_envelope(
                        seq_hint.saturating_add(1),
                        &request_id,
                        error.close_reason(),
                    );
                    let data = serde_json::to_string(&closed).unwrap_or_else(|_| "{}".into());
                    let _ = tokio::time::timeout(
                        SEND_TIMEOUT,
                        tx.send(
                            Event::default()
                                .id(closed.sequence.to_string())
                                .event(closed.event)
                                .data(data),
                        ),
                    )
                    .await;
                    return;
                }
            }
            for envelope in chunk {
                seq_hint = envelope.sequence;
                let data = serde_json::to_string(envelope).unwrap_or_else(|_| "{}".into());
                let event = Event::default()
                    .id(envelope.sequence.to_string())
                    .event(envelope.event.clone())
                    .data(data);
                match tokio::time::timeout(SEND_TIMEOUT, tx.send(event)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => return,
                    Err(_) => {
                        let closed = auth_closed_envelope(
                            seq_hint.saturating_add(1),
                            &request_id,
                            crate::services::stream_auth::StreamAuthError::SendTimeout
                                .close_reason(),
                        );
                        let data = serde_json::to_string(&closed).unwrap_or_else(|_| "{}".into());
                        let _ = tx
                            .send(
                                Event::default()
                                    .id(closed.sequence.to_string())
                                    .event(closed.event)
                                    .data(data),
                            )
                            .await;
                        return;
                    }
                }
            }
        }
        let closed = auth_closed_envelope(seq_hint.saturating_add(1), &request_id, "completed");
        let data = serde_json::to_string(&closed).unwrap_or_else(|_| "{}".into());
        let _ = tokio::time::timeout(
            SEND_TIMEOUT,
            tx.send(
                Event::default()
                    .id(closed.sequence.to_string())
                    .event(closed.event)
                    .data(data),
            ),
        )
        .await;
    });

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|event| (Ok::<_, Infallible>(event), rx))
    });
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("heartbeat"),
    ))
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
    Unavailable(String),
    Database(String),
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

// Silence unused import warning if feature sets change.
#[allow(dead_code)]
fn _sse_helper() {
    let _ = into_sse_stream(Vec::new());
}
