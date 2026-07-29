//! Project CRUD (P2-18): org-scoped folders of collections.
//!
//! Permission precedent mirrors `routes::collections` exactly (see that
//! module + the P2-18 session report for the full rationale): collection
//! create/update there is gated by `doc.upload` (the existing "who may
//! manage the library's structure" permission — editor/admin/owner all hold
//! it; there is no separate `project.manage` permission and this slice does
//! not add one). Project create/update/collection-assign reuse the same
//! `doc.upload` gate rather than inventing a parallel permission for a
//! sibling piece of library structure. Project deletion is out of scope for
//! this slice (see the P2-18 backlog entry) — no route exists for it here.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::{ApiError, Page, PageInfo};
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::error::DbError;
use crate::db::pool::with_org_txn;
use crate::db::projects;
use crate::http::AppState;
use crate::services::audit;

/// Same gate `routes::collections::{create_collection, update_collection}`
/// use — see this module's doc for why no separate permission was added.
const PERMISSION_PROJECT_MANAGE: &str = "doc.upload";

const MAX_NAME_LEN: usize = 200;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/projects", get(list_projects).post(create_project))
        .route(
            "/api/v1/projects/{project_id}",
            axum::routing::patch(update_project),
        )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDto {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectRequest {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProjectRequest {
    name: String,
}

fn project_dto(project: crate::db::models::Project) -> ProjectDto {
    ProjectDto {
        id: project.id,
        name: project.name,
        created_at: project.created_at,
    }
}

fn validate_name(name: &str) -> Option<&str> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_NAME_LEN {
        None
    } else {
        Some(trimmed)
    }
}

// ---------------------------------------------------------------------
// GET /projects — every project in the caller's org ("all projects" in the
// UI/API is the absence of a filter, never a row here — no list-scoping
// beyond org isolation is needed).
// ---------------------------------------------------------------------

async fn list_projects(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
) -> Result<Json<Page<ProjectDto>>, RouteError> {
    let items = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| Box::pin(async move { projects::list(txn, &ctx).await })
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(Json(Page {
        items: items.into_iter().map(project_dto).collect(),
        page: PageInfo {
            next_cursor: None,
            has_more: false,
        },
    }))
}

// ---------------------------------------------------------------------
// POST /projects
// ---------------------------------------------------------------------

async fn create_project(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectDto>), RouteError> {
    if require_permission(&auth.context, PERMISSION_PROJECT_MANAGE).is_err() {
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "project.create",
            "project",
            None,
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let Some(name) = validate_name(&body.name) else {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid project name",
        ));
    };
    let id = Uuid::new_v4();
    let name = name.to_string();
    let request_id = auth.request_id.clone();
    let project = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                let project = projects::insert(txn, &ctx, id, &name).await?;
                let resource_id = project.id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    audit::AuditRecord {
                        request_id: &request_id,
                        action: "project.create",
                        resource_type: "project",
                        resource_id: Some(&resource_id),
                        outcome: crate::db::models::AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "project_id": project.id.to_string(),
                            "name_chars": project.name.len() as i64,
                        }),
                    },
                )
                .await?;
                Ok(project)
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok((StatusCode::CREATED, Json(project_dto(project))))
}

// ---------------------------------------------------------------------
// PATCH /projects/{projectId} — rename only (no other mutable field yet).
// ---------------------------------------------------------------------

async fn update_project(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(project_id): Path<Uuid>,
    Json(body): Json<UpdateProjectRequest>,
) -> Result<Json<ProjectDto>, RouteError> {
    if require_permission(&auth.context, PERMISSION_PROJECT_MANAGE).is_err() {
        let resource_id = project_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "project.update",
            "project",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    let Some(name) = validate_name(&body.name) else {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid project name",
        ));
    };
    let name = name.to_string();
    let request_id = auth.request_id.clone();
    let project = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                let project = projects::update_name(txn, &ctx, project_id, &name).await?;
                let resource_id = project.id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    audit::AuditRecord {
                        request_id: &request_id,
                        action: "project.update",
                        resource_type: "project",
                        resource_id: Some(&resource_id),
                        outcome: crate::db::models::AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "project_id": project.id.to_string(),
                            "name_chars": project.name.len() as i64,
                        }),
                    },
                )
                .await?;
                Ok(project)
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(Json(project_dto(project)))
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
                "Project not found",
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
