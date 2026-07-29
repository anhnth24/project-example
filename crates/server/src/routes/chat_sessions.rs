//! Private per-user Q&A chat history (P2-19).
//!
//! Gated by `qa.query` — the same permission `routes::search`/`routes::ask`
//! require, since this is the same Q&A surface (owner request: reuse, don't
//! invent a `chat.manage` permission for a sibling piece of the same
//! feature — same rationale `routes::projects` documents for reusing
//! `doc.upload` instead of a new `project.manage`).
//!
//! Every route is scoped to the caller's own sessions: `db::chat_sessions`
//! filters `user_id = caller` on top of org RLS, so a session belonging to
//! another user (even in the same org) 404s identically to one that never
//! existed — no endpoint here can read, list, rename, append to, or delete
//! another user's chat history, ever.
//!
//! Citations/warnings are accepted and stored as opaque JSON (the exact
//! payload the client already rendered from its own `/ask` or `/ask/stream`
//! response) and are never re-validated/re-authorized here — only checked
//! for basic shape (must be a JSON array) so a turn can't persist a
//! non-array value. The client re-validates a citation for real (hash/span/
//! ACL) via `POST /citations/resolve` when the user actually clicks a
//! deep-link out of history — this is a deliberate trade-off: chat history
//! is a client-side record of what was shown, not a second source of
//! citation authority.
//!
//! Turn content (question/answer) is deliberately NOT audited — only
//! `chat_session.create`/`chat_session.delete` are, and only with
//! structural metadata (`session_id`), never question/answer/title text
//! (see `services::audit::AuditAction::ChatSessionCreate`'s allowlist and
//! `FORBIDDEN_METADATA_KEYS`). Renaming a session and appending a turn are
//! not audited at all, matching the owner's spec for this slice.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::api::{decode_cursor, encode_cursor, ApiError, Page, PageInfo, Pagination};
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::chat_sessions::{
    self, ChatSession, ChatTurn, NewChatTurn, ALLOWED_ANSWER_MODES, MAX_QUESTION_LEN, MAX_TITLE_LEN,
};
use crate::db::error::DbError;
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::services::audit;
use crate::services::retrieval::PERMISSION_QA_QUERY;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/chat-sessions",
            get(list_chat_sessions).post(create_chat_session),
        )
        .route(
            "/api/v1/chat-sessions/{session_id}",
            get(get_chat_session)
                .patch(update_chat_session)
                .delete(delete_chat_session),
        )
        .route(
            "/api/v1/chat-sessions/{session_id}/turns",
            post(append_chat_turn),
        )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatSessionDto {
    id: Uuid,
    title: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatTurnDto {
    id: Uuid,
    seq: i32,
    question: String,
    answer: String,
    answer_mode: String,
    citations: JsonValue,
    warnings: JsonValue,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatSessionDetailDto {
    #[serde(flatten)]
    session: ChatSessionDto,
    turns: Vec<ChatTurnDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateChatSessionRequest {
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateChatSessionRequest {
    title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppendChatTurnRequest {
    question: String,
    answer: String,
    answer_mode: String,
    #[serde(default = "default_json_array")]
    citations: JsonValue,
    #[serde(default = "default_json_array")]
    warnings: JsonValue,
}

fn default_json_array() -> JsonValue {
    JsonValue::Array(Vec::new())
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

fn session_dto(session: ChatSession) -> ChatSessionDto {
    ChatSessionDto {
        id: session.id,
        title: session.title,
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

fn turn_dto(turn: ChatTurn) -> ChatTurnDto {
    ChatTurnDto {
        id: turn.id,
        seq: turn.seq,
        question: turn.question,
        answer: turn.answer,
        answer_mode: turn.answer_mode,
        citations: turn.citations,
        warnings: turn.warnings,
        created_at: turn.created_at,
    }
}

fn validate_title(title: &str) -> Option<&str> {
    let trimmed = title.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_TITLE_LEN {
        None
    } else {
        Some(trimmed)
    }
}

fn validate_question(question: &str) -> Option<&str> {
    let trimmed = question.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_QUESTION_LEN {
        None
    } else {
        Some(trimmed)
    }
}

fn validate_answer_mode(mode: &str) -> bool {
    ALLOWED_ANSWER_MODES.contains(&mode)
}

// ---------------------------------------------------------------------
// GET /chat-sessions — the caller's own sessions, most recently active first.
// ---------------------------------------------------------------------

async fn list_chat_sessions(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<ChatSessionDto>>, RouteError> {
    if require_permission(&auth.context, PERMISSION_QA_QUERY).is_err() {
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let pagination = Pagination::from_query(query.limit);
    let (after_updated_at, after_id) = match query.cursor.as_deref() {
        Some(raw) => decode_cursor(raw)
            .map(|(at, id)| (Some(at), Some(id)))
            .ok_or_else(|| RouteError::Validation(auth.request_id.clone(), "Invalid cursor"))?,
        None => (None, None),
    };
    let mut rows = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                chat_sessions::list_owned_sessions_page(
                    txn,
                    &ctx,
                    pagination.limit + 1,
                    after_updated_at,
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
    let next_cursor = rows.last().map(|row| encode_cursor(row.updated_at, row.id));

    Ok(Json(Page {
        items: rows.into_iter().map(session_dto).collect(),
        page: PageInfo {
            next_cursor,
            has_more,
        },
    }))
}

// ---------------------------------------------------------------------
// POST /chat-sessions
// ---------------------------------------------------------------------

async fn create_chat_session(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<CreateChatSessionRequest>,
) -> Result<(StatusCode, Json<ChatSessionDto>), RouteError> {
    if require_permission(&auth.context, PERMISSION_QA_QUERY).is_err() {
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            audit::AuditAction::ChatSessionCreate.as_str(),
            "chat_session",
            None,
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let Some(title) = validate_title(&body.title) else {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid title",
        ));
    };
    let id = Uuid::new_v4();
    let title = title.to_string();
    let request_id = auth.request_id.clone();
    let session = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                let session = chat_sessions::insert_session(txn, &ctx, id, &title).await?;
                let resource_id = session.id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    audit::AuditRecord {
                        request_id: &request_id,
                        action: audit::AuditAction::ChatSessionCreate.as_str(),
                        resource_type: "chat_session",
                        resource_id: Some(&resource_id),
                        outcome: crate::db::models::AuditOutcome::Success,
                        metadata: serde_json::json!({ "session_id": session.id.to_string() }),
                    },
                )
                .await?;
                Ok(session)
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok((StatusCode::CREATED, Json(session_dto(session))))
}

// ---------------------------------------------------------------------
// GET /chat-sessions/{sessionId} — detail with turns, ordered by seq.
// ---------------------------------------------------------------------

async fn get_chat_session(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(session_id): Path<Uuid>,
) -> Result<Json<ChatSessionDetailDto>, RouteError> {
    if require_permission(&auth.context, PERMISSION_QA_QUERY).is_err() {
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let (session, turns) = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                let session = chat_sessions::get_owned_session(txn, &ctx, session_id).await?;
                let turns = chat_sessions::list_turns(txn, &ctx, session_id).await?;
                Ok((session, turns))
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(Json(ChatSessionDetailDto {
        session: session_dto(session),
        turns: turns.into_iter().map(turn_dto).collect(),
    }))
}

// ---------------------------------------------------------------------
// PATCH /chat-sessions/{sessionId} — rename only.
// ---------------------------------------------------------------------

async fn update_chat_session(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(session_id): Path<Uuid>,
    Json(body): Json<UpdateChatSessionRequest>,
) -> Result<Json<ChatSessionDto>, RouteError> {
    if require_permission(&auth.context, PERMISSION_QA_QUERY).is_err() {
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let Some(title) = validate_title(&body.title) else {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid title",
        ));
    };
    let title = title.to_string();
    let session = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(
                async move { chat_sessions::update_title(txn, &ctx, session_id, &title).await },
            )
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(Json(session_dto(session)))
}

// ---------------------------------------------------------------------
// DELETE /chat-sessions/{sessionId} — hard delete, turns cascade.
// ---------------------------------------------------------------------

async fn delete_chat_session(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(session_id): Path<Uuid>,
) -> Result<StatusCode, RouteError> {
    if require_permission(&auth.context, PERMISSION_QA_QUERY).is_err() {
        let resource_id = session_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            audit::AuditAction::ChatSessionDelete.as_str(),
            "chat_session",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let request_id = auth.request_id.clone();
    with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                chat_sessions::delete_owned_session(txn, &ctx, session_id).await?;
                let resource_id = session_id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    audit::AuditRecord {
                        request_id: &request_id,
                        action: audit::AuditAction::ChatSessionDelete.as_str(),
                        resource_type: "chat_session",
                        resource_id: Some(&resource_id),
                        outcome: crate::db::models::AuditOutcome::Success,
                        metadata: serde_json::json!({ "session_id": session_id.to_string() }),
                    },
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------
// POST /chat-sessions/{sessionId}/turns — client appends after its own
// stream/JSON /ask already completed and rendered the answer.
// ---------------------------------------------------------------------

async fn append_chat_turn(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(session_id): Path<Uuid>,
    Json(body): Json<AppendChatTurnRequest>,
) -> Result<(StatusCode, Json<ChatTurnDto>), RouteError> {
    if require_permission(&auth.context, PERMISSION_QA_QUERY).is_err() {
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let Some(question) = validate_question(&body.question) else {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid question",
        ));
    };
    if !validate_answer_mode(&body.answer_mode) {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid answerMode",
        ));
    }
    if !body.citations.is_array() {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "citations must be an array",
        ));
    }
    if !body.warnings.is_array() {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "warnings must be an array",
        ));
    }
    let input = NewChatTurn {
        question: question.to_string(),
        answer: body.answer,
        answer_mode: body.answer_mode,
        citations: body.citations,
        warnings: body.warnings,
    };
    let turn = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move { chat_sessions::append_turn(txn, &ctx, session_id, input).await })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok((StatusCode::CREATED, Json(turn_dto(turn))))
}

enum RouteError {
    Denied(String),
    Validation(String, &'static str),
    NotFound(String),
    Database(String),
}

impl RouteError {
    fn from_db(error: DbError, request_id: &str) -> Self {
        match error {
            DbError::NotFound => Self::NotFound(request_id.to_string()),
            _ => Self::Database(request_id.to_string()),
        }
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
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
                "Chat session not found",
                request_id,
            ),
            Self::Database(request_id) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Request failed",
                request_id,
            ),
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
