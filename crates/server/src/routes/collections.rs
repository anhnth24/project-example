//! Collection CRUD for the single-org POC (P1B-R04).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::{ApiError, CollectionDto, Page, PageInfo};
use crate::auth::middleware::AuthenticatedOrg;
use crate::auth::permissions::require_permission;
use crate::db::collections::{self, NewCollection};
use crate::db::error::DbError;
use crate::db::models::CollectionVisibility;
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::services::audit;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/collections",
            get(list_collections).post(create_collection),
        )
        .route(
            "/api/v1/collections/{collection_id}",
            get(get_collection)
                .patch(update_collection)
                .delete(delete_collection),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCollectionRequest {
    name: String,
    slug: String,
    description: Option<String>,
    #[serde(default = "default_visibility")]
    visibility: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCollectionRequest {
    name: String,
    description: Option<String>,
}

fn default_visibility() -> String {
    "org".into()
}

async fn list_collections(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
) -> Result<Json<Page<CollectionDto>>, RouteError> {
    let items = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                let rows = collections::list(txn, &ctx).await?;
                Ok(rows
                    .into_iter()
                    .filter(|row| ctx.allows_collection(row.id))
                    .map(|row| CollectionDto {
                        id: row.id,
                        name: row.name,
                        slug: row.slug,
                        description: row.description,
                        visibility: row.visibility.as_str().into(),
                        created_at: row.created_at,
                    })
                    .collect::<Vec<_>>())
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(Json(Page {
        items,
        page: PageInfo {
            next_cursor: None,
            has_more: false,
        },
    }))
}

async fn get_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(collection_id): Path<Uuid>,
) -> Result<Json<CollectionDto>, RouteError> {
    if !auth.context.allows_collection(collection_id) {
        return Err(RouteError::NotFound(auth.request_id));
    }
    let row = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| Box::pin(async move { collections::get_by_id(txn, &ctx, collection_id).await })
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(Json(CollectionDto {
        id: row.id,
        name: row.name,
        slug: row.slug,
        description: row.description,
        visibility: row.visibility.as_str().into(),
        created_at: row.created_at,
    }))
}

async fn update_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(collection_id): Path<Uuid>,
    Json(body): Json<UpdateCollectionRequest>,
) -> Result<Json<CollectionDto>, RouteError> {
    if require_permission(&auth.context, "doc.upload").is_err() {
        let resource_id = collection_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "collection.update",
            "collection",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    if !auth.context.allows_collection(collection_id) {
        return Err(RouteError::NotFound(auth.request_id));
    }
    if body.name.trim().is_empty() || body.name.len() > 200 {
        let resource_id = collection_id.to_string();
        audit::record(
            state.pool(),
            &auth.context,
            audit::AuditRecord {
                request_id: &auth.request_id,
                action: "collection.update",
                resource_type: "collection",
                resource_id: Some(&resource_id),
                outcome: crate::db::models::AuditOutcome::Error,
                metadata: serde_json::json!({
                    "reason": "validation_failed",
                }),
            },
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid collection name",
        ));
    }
    let request_id = auth.request_id.clone();
    let row = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let name = body.name.trim().to_string();
        let description = body.description.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                let row = collections::update_metadata(
                    txn,
                    &ctx,
                    collection_id,
                    &name,
                    description.as_deref(),
                )
                .await?;
                let resource_id = row.id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    audit::AuditRecord {
                        request_id: &request_id,
                        action: "collection.update",
                        resource_type: "collection",
                        resource_id: Some(&resource_id),
                        outcome: crate::db::models::AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "collection_id": row.id.to_string(),
                            "name_chars": row.name.len() as i64,
                        }),
                    },
                )
                .await?;
                Ok(row)
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(Json(CollectionDto {
        id: row.id,
        name: row.name,
        slug: row.slug,
        description: row.description,
        visibility: row.visibility.as_str().into(),
        created_at: row.created_at,
    }))
}

async fn delete_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(collection_id): Path<Uuid>,
) -> Result<StatusCode, RouteError> {
    if require_permission(&auth.context, "doc.delete").is_err() {
        let resource_id = collection_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "collection.delete",
            "collection",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    if !auth.context.allows_collection(collection_id) {
        return Err(RouteError::NotFound(auth.request_id));
    }
    let request_id = auth.request_id.clone();
    with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                collections::soft_delete(txn, &ctx, collection_id).await?;
                let resource_id = collection_id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    audit::AuditRecord {
                        request_id: &request_id,
                        action: "collection.delete",
                        resource_type: "collection",
                        resource_id: Some(&resource_id),
                        outcome: crate::db::models::AuditOutcome::Success,
                        metadata: serde_json::json!({}),
                    },
                )
                .await?;
                Ok(())
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<CreateCollectionRequest>,
) -> Result<(StatusCode, Json<CollectionDto>), RouteError> {
    if require_permission(&auth.context, "doc.upload").is_err() {
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            "collection.create",
            "collection",
            None,
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }
    if body.name.trim().is_empty()
        || body.name.len() > 200
        || body.slug.trim().is_empty()
        || body.slug.len() > 80
    {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid collection name or slug",
        ));
    }
    let visibility = CollectionVisibility::parse(&body.visibility).map_err(|_| {
        RouteError::Validation(auth.request_id.clone(), "Invalid collection visibility")
    })?;
    let id = Uuid::new_v4();
    let request_id = auth.request_id.clone();
    let row = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let name = body.name.trim().to_string();
        let slug = body.slug.trim().to_string();
        let description = body.description.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                let row = collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id,
                        name: &name,
                        slug: &slug,
                        description: description.as_deref(),
                        visibility,
                    },
                )
                .await?;
                let resource_id = row.id.to_string();
                audit::record_in_txn(
                    txn,
                    &ctx,
                    audit::AuditRecord {
                        request_id: &request_id,
                        action: "collection.create",
                        resource_type: "collection",
                        resource_id: Some(&resource_id),
                        outcome: crate::db::models::AuditOutcome::Success,
                        metadata: serde_json::json!({
                            "collection_id": row.id.to_string(),
                            "name_chars": row.name.len() as i64,
                            "slug_chars": row.slug.len() as i64,
                        }),
                    },
                )
                .await?;
                Ok(row)
            })
        }
    })
    .await
    .map_err(|error| RouteError::from_db(error, &auth.request_id))?;
    Ok((
        StatusCode::CREATED,
        Json(CollectionDto {
            id: row.id,
            name: row.name,
            slug: row.slug,
            description: row.description,
            visibility: row.visibility.as_str().into(),
            created_at: row.created_at,
        }),
    ))
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
            DbError::Config(message) if message == "collection_denied" => {
                Self::NotFound(request_id.to_string())
            }
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
                "Collection not found",
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
