//! GET /api/v1/audit — org-scoped, permission-gated audit log read (1C-11).
//!
//! `audit_log` is append-only and write-only up to this slice (writes go
//! through `services::audit` / `auth::session::write_audit`, co-committed
//! with the mutation they describe — see `db/audit.rs` module doc). This is
//! the first and only *read* surface. Scope is deliberately narrow: list with
//! cursor pagination + `action`/`actor`/`from`/`to` filters, org isolation via
//! `org_id = $1` (defense-in-depth; RLS `audit_log_org_isolation` from
//! migration `0010` is the real gate), and the `audit.view` permission guard.
//! No retention/export/SIEM surface here — see the 1C-11 backlog entry for
//! what remains out of scope.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::{
    decode_cursor, encode_cursor, ApiError, AuditEntryDto, Page, PageInfo, Pagination,
};
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::audit::AuditListFilter;
use crate::db::models::AuditLogEntry;
use crate::http::AppState;
use crate::services::audit::{self, AuditAction};
use crate::services::audit_query::{self, AuditQueryError};

/// Seeded in `migrations/0011` (POC catalog) and `migrations/0030` (global
/// role catalog) — owner/admin roles hold it by default, matching the task's
/// "already seeded, owner/admin have it" note.
const PERMISSION_AUDIT_VIEW: &str = "audit.view";

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/audit", get(list_audit))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
    action: Option<String>,
    actor: Option<Uuid>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

fn audit_entry_dto(row: AuditLogEntry) -> AuditEntryDto {
    AuditEntryDto {
        id: row.id,
        seq: row.seq,
        actor_id: row.actor_user_id,
        action: row.action,
        target_type: row.resource_type,
        target_id: row.resource_id,
        outcome: row.outcome.as_str().into(),
        metadata: row.metadata,
        request_id: row.request_id,
        occurred_at: row.created_at,
    }
}

async fn list_audit(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<AuditEntryDto>>, RouteError> {
    if require_permission(&auth.context, PERMISSION_AUDIT_VIEW).is_err() {
        // Reading the audit trail is itself sensitive enough to audit the
        // denial (unlike plain list_members/list_invites/usage, which do
        // not audit at all) — mirrors search.query/ask.query's deny audit.
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            AuditAction::AuditRead.as_str(),
            "audit",
            None,
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }

    // Exact-match action filter validated against the closed action enum —
    // same "reject unknown enum value up front" convention as
    // MembershipRole::parse in routes/members.rs. Never a free-text LIKE.
    let action = match query.action.as_deref() {
        Some(raw) => Some(
            AuditAction::parse(raw)
                .map_err(|_| RouteError::Validation(auth.request_id.clone(), "Invalid action"))?
                .as_str()
                .to_string(),
        ),
        None => None,
    };

    if let (Some(from), Some(to)) = (query.from, query.to) {
        if from > to {
            return Err(RouteError::Validation(
                auth.request_id.clone(),
                "from must not be after to",
            ));
        }
    }

    let pagination = Pagination::from_query(query.limit);
    let (after_at, after_id) = match query.cursor.as_deref() {
        Some(raw) => decode_cursor(raw)
            .map(|(at, id)| (Some(at), Some(id)))
            .ok_or_else(|| RouteError::Validation(auth.request_id.clone(), "Invalid cursor"))?,
        None => (None, None),
    };

    let filter = AuditListFilter {
        action,
        actor_user_id: query.actor,
        from: query.from,
        to: query.to,
    };

    let mut rows = audit_query::list_page(
        state.pool(),
        &auth.context,
        &filter,
        pagination.limit + 1,
        after_at,
        after_id,
    )
    .await
    .map_err(|error| match error {
        // Route already returned 403 on missing audit.view; dual-layer deny
        // still maps to forbidden rather than inventing a new shape.
        AuditQueryError::PermissionDenied => RouteError::Denied(auth.request_id.clone()),
        AuditQueryError::Database => RouteError::Database(auth.request_id.clone()),
    })?;

    let has_more = rows.len() as i64 > pagination.limit;
    if has_more {
        rows.truncate(pagination.limit as usize);
    }
    let next_cursor = rows.last().map(|row| encode_cursor(row.created_at, row.id));
    let result_count = rows.len();

    audit::record(
        state.pool(),
        &auth.context,
        audit::AuditRecord {
            request_id: &auth.request_id,
            action: AuditAction::AuditRead.as_str(),
            resource_type: "audit",
            resource_id: None,
            outcome: crate::db::models::AuditOutcome::Success,
            metadata: serde_json::json!({ "result_count": result_count as i64 }),
        },
    )
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;

    Ok(Json(Page {
        items: rows.into_iter().map(audit_entry_dto).collect(),
        page: PageInfo {
            next_cursor,
            has_more,
        },
    }))
}

enum RouteError {
    Denied(String),
    Validation(String, &'static str),
    Database(String),
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
