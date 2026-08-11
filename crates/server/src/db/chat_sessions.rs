//! Private per-user Q&A chat history (P2-19, migrations/0034).
//!
//! Every function here takes `ctx.user_id()` as an *additional* filter on
//! top of the org-isolation RLS policy (`qa_chat_sessions_org_isolation` /
//! `qa_chat_turns_org_isolation`) — same "RLS is org isolation, ownership is
//! an extra WHERE clause" pattern `db::ask_streams::get_owned_session`
//! already established. There is no endpoint anywhere that reads, lists, or
//! mutates another user's session, even within the same org: every lookup
//! below 404s (via [`crate::db::error::DbError::NotFound`]) for an id that
//! belongs to another user or another org — RLS makes the two
//! indistinguishable from the caller's point of view, which is the point.

use chrono::{DateTime, Utc};
use serde_json::Value as JsonValue;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::db::error::DbError;

/// Answer modes a client may report when appending a turn — mirrors
/// `fileconv_knowledge::ask::AnswerMode::as_str()` exactly (kept as a plain
/// string allowlist here, not a cross-crate type, since this is purely a
/// client-supplied record of what the client already displayed).
pub const ALLOWED_ANSWER_MODES: &[&str] = &[
    "offline_extractive",
    "fallback_extractive",
    "local_llm",
    "cloud_llm",
    "subscription_cli",
    "llm_unverified",
    "assistant",
];

pub const MAX_TITLE_LEN: usize = 200;
pub const MAX_QUESTION_LEN: usize = 8_192;

#[derive(Debug, Clone, PartialEq)]
pub struct ChatSession {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatTurn {
    pub id: Uuid,
    pub session_id: Uuid,
    pub org_id: Uuid,
    pub seq: i32,
    pub question: String,
    pub answer: String,
    pub answer_mode: String,
    pub citations: JsonValue,
    pub warnings: JsonValue,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewChatTurn {
    pub question: String,
    pub answer: String,
    pub answer_mode: String,
    pub citations: JsonValue,
    pub warnings: JsonValue,
}

const SESSION_COLUMNS: &str = "id, org_id, user_id, title, created_at, updated_at";
const TURN_COLUMNS: &str =
    "id, session_id, org_id, seq, question, answer, answer_mode, citations, warnings, created_at";

/// Creates a session owned by the caller.
pub async fn insert_session(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    id: Uuid,
    title: &str,
) -> Result<ChatSession, DbError> {
    let row = txn
        .query_one(
            &format!(
                "INSERT INTO qa_chat_sessions (id, org_id, user_id, title)
                 VALUES ($1, $2, $3, $4)
                 RETURNING {SESSION_COLUMNS}"
            ),
            &[&id, &ctx.org_id(), &ctx.user_id(), &title],
        )
        .await?;
    Ok(map_session(&row))
}

/// Cursor-paginated sessions owned by the caller, most recently active
/// first — appending a turn (or renaming) bumps `updated_at`. Same
/// `(sort_key, id)` tuple-cursor convention as `db::audit::list_page`.
pub async fn list_owned_sessions_page(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    limit: i64,
    after_updated_at: Option<DateTime<Utc>>,
    after_id: Option<Uuid>,
) -> Result<Vec<ChatSession>, DbError> {
    let limit = limit.clamp(1, 101);
    let rows = txn
        .query(
            &format!(
                "SELECT {SESSION_COLUMNS}
                 FROM qa_chat_sessions
                 WHERE org_id = $1 AND user_id = $2
                   AND (
                        $3::timestamptz IS NULL
                        OR (updated_at, id) < ($3::timestamptz, $4::uuid)
                   )
                 ORDER BY updated_at DESC, id DESC
                 LIMIT $5"
            ),
            &[
                &ctx.org_id(),
                &ctx.user_id(),
                &after_updated_at,
                &after_id,
                &limit,
            ],
        )
        .await?;
    Ok(rows.iter().map(map_session).collect())
}

/// Fetches one session, scoped to the tenant *and* the caller — a session
/// belonging to another user (even in the same org) or another org is
/// `DbError::NotFound`, indistinguishable from "never existed".
pub async fn get_owned_session(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
) -> Result<ChatSession, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "SELECT {SESSION_COLUMNS}
                 FROM qa_chat_sessions
                 WHERE org_id = $1 AND id = $2 AND user_id = $3"
            ),
            &[&ctx.org_id(), &session_id, &ctx.user_id()],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(map_session(&row))
}

/// Renames a session owned by the caller.
pub async fn update_title(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
    title: &str,
) -> Result<ChatSession, DbError> {
    let row = txn
        .query_opt(
            &format!(
                "UPDATE qa_chat_sessions
                 SET title = $4, updated_at = now()
                 WHERE org_id = $1 AND id = $2 AND user_id = $3
                 RETURNING {SESSION_COLUMNS}"
            ),
            &[&ctx.org_id(), &session_id, &ctx.user_id(), &title],
        )
        .await?
        .ok_or(DbError::NotFound)?;
    Ok(map_session(&row))
}

/// Hard-deletes a session owned by the caller; turns cascade
/// (`fk_qa_chat_turns__session ... ON DELETE CASCADE`, migrations/0034).
pub async fn delete_owned_session(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
) -> Result<(), DbError> {
    let deleted = txn
        .execute(
            "DELETE FROM qa_chat_sessions WHERE org_id = $1 AND id = $2 AND user_id = $3",
            &[&ctx.org_id(), &session_id, &ctx.user_id()],
        )
        .await?;
    if deleted == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

/// All turns of a session owned by the caller, ordered by `seq`. Callers
/// must first (or atomically) confirm session ownership via
/// [`get_owned_session`] — this alone does not 404 for an empty result vs an
/// unowned session, since a real, owned, empty-turn session is valid.
pub async fn list_turns(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
) -> Result<Vec<ChatTurn>, DbError> {
    let rows = txn
        .query(
            &format!(
                "SELECT {TURN_COLUMNS}
                 FROM qa_chat_turns
                 WHERE org_id = $1 AND session_id = $2 AND user_id = $3
                 ORDER BY seq ASC"
            ),
            &[&ctx.org_id(), &session_id, &ctx.user_id()],
        )
        .await?;
    rows.iter().map(map_turn).collect()
}

/// Appends the next turn to a session owned by the caller. `seq` is
/// server-assigned as `max(seq) + 1` within this transaction, under a
/// `FOR UPDATE` lock on the session row so two concurrent appends to the
/// same session never race to the same `seq` (which `uq_qa_chat_turns__session_seq`
/// would otherwise reject one of as a constraint violation instead of
/// serializing them).
pub async fn append_turn(
    txn: &Transaction<'_>,
    ctx: &OrgContext,
    session_id: Uuid,
    input: NewChatTurn,
) -> Result<ChatTurn, DbError> {
    let owned = txn
        .query_opt(
            "SELECT id FROM qa_chat_sessions
             WHERE org_id = $1 AND id = $2 AND user_id = $3
             FOR UPDATE",
            &[&ctx.org_id(), &session_id, &ctx.user_id()],
        )
        .await?;
    if owned.is_none() {
        return Err(DbError::NotFound);
    }
    let next_seq: i32 = txn
        .query_one(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM qa_chat_turns
             WHERE org_id = $1 AND session_id = $2",
            &[&ctx.org_id(), &session_id],
        )
        .await?
        .get(0);
    let row = txn
        .query_one(
            &format!(
                "INSERT INTO qa_chat_turns (
                    id, session_id, org_id, user_id, seq, question, answer,
                    answer_mode, citations, warnings
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 RETURNING {TURN_COLUMNS}"
            ),
            &[
                &Uuid::new_v4(),
                &session_id,
                &ctx.org_id(),
                &ctx.user_id(),
                &next_seq,
                &input.question,
                &input.answer,
                &input.answer_mode,
                &input.citations,
                &input.warnings,
            ],
        )
        .await?;
    txn.execute(
        "UPDATE qa_chat_sessions SET updated_at = now()
         WHERE org_id = $1 AND id = $2 AND user_id = $3",
        &[&ctx.org_id(), &session_id, &ctx.user_id()],
    )
    .await?;
    map_turn(&row)
}

fn map_session(row: &Row) -> ChatSession {
    ChatSession {
        id: row.get("id"),
        org_id: row.get("org_id"),
        user_id: row.get("user_id"),
        title: row.get("title"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn map_turn(row: &Row) -> Result<ChatTurn, DbError> {
    Ok(ChatTurn {
        id: row.get("id"),
        session_id: row.get("session_id"),
        org_id: row.get("org_id"),
        seq: row.get("seq"),
        question: row.get("question"),
        answer: row.get("answer"),
        answer_mode: row.get("answer_mode"),
        citations: row.get("citations"),
        warnings: row.get("warnings"),
        created_at: row.get("created_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_answer_modes_match_known_set() {
        assert!(ALLOWED_ANSWER_MODES.contains(&"offline_extractive"));
        assert!(ALLOWED_ANSWER_MODES.contains(&"llm_unverified"));
        assert!(ALLOWED_ANSWER_MODES.contains(&"assistant"));
        assert!(!ALLOWED_ANSWER_MODES.contains(&"bogus_mode"));
    }
}
