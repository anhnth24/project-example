//! Collection POC REST routes (`/api/v1/collections`).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::{
    decode_cursor, encode_cursor, ApiRejection, AppJson, AppPath, AppQuery, CollectionResponse,
    ListResponse, NameIdCursor, PageInfo, PageParams,
};
use crate::auth::middleware::AuthenticatedOrg;
use crate::db::collections::{self, NewCollection, UpdateCollection};
use crate::db::models::CollectionVisibility;
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::routes::common::{
    collection_response, deny_or_not_found, map_db, parse_slug, require_coll, require_perm,
};

/// Canonical write permission for collection create/update (editor+; viewers denied).
const COLLECTION_WRITE_PERM: &str = "doc.upload";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/collections",
            get(list_collections).post(create_collection),
        )
        .route(
            "/api/v1/collections/{collection_id}",
            get(get_collection).patch(update_collection),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    name: String,
    slug: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<Option<String>>,
    #[serde(default)]
    visibility: Option<String>,
}

async fn list_collections(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppQuery(query): AppQuery<ListQuery>,
) -> Result<Json<ListResponse<CollectionResponse>>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let page = PageParams::from_query(query.limit, query.cursor, &request_id)?;
    let after = match page.cursor.as_deref() {
        Some(raw) => Some(decode_cursor::<NameIdCursor>(raw).map_err(|message| {
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
        let after_name = after.as_ref().map(|cursor| cursor.name.clone());
        let after_id = after.as_ref().map(|cursor| cursor.id);
        move |txn| {
            Box::pin(async move {
                collections::list_allowed_page(
                    txn,
                    &ctx,
                    &allowed,
                    fetch_limit,
                    after_name.as_deref(),
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
            encode_cursor(&NameIdCursor {
                name: row.name.clone(),
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
            .map(|row| collection_response(row, request_id.clone()))
            .collect(),
        page_info,
        request_id,
    }))
}

async fn get_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(collection_id): AppPath<Uuid>,
) -> Result<Json<CollectionResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_coll(&auth.context, collection_id, &request_id)
        .map_err(|_| deny_or_not_found(&request_id))?;
    let collection = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| Box::pin(async move { collections::get_by_id(txn, &ctx, collection_id).await })
    })
    .await
    .map_err(|error| map_db(error, &request_id))?;
    Ok(Json(collection_response(collection, request_id)))
}

async fn create_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppJson(body): AppJson<CreateBody>,
) -> Result<(axum::http::StatusCode, Json<CollectionResponse>), ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, COLLECTION_WRITE_PERM, &request_id)?;
    if body.name.trim().is_empty() || body.name.len() > 200 {
        return Err(ApiRejection::validation(
            "name must be 1..=200 characters",
            &request_id,
        ));
    }
    let slug = parse_slug(body.slug.trim())
        .map_err(|message| ApiRejection::validation(message, &request_id))?
        .to_string();
    let visibility = match body.visibility.as_deref() {
        None | Some("org") => CollectionVisibility::Org,
        Some("private") => CollectionVisibility::Private,
        Some("groups") => CollectionVisibility::Groups,
        Some(_) => {
            return Err(ApiRejection::validation(
                "visibility must be one of org|private|groups",
                &request_id,
            ))
        }
    };
    let id = Uuid::new_v4();
    let name = body.name.trim().to_string();
    let description = body
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let collection = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let description = description.clone();
        let slug = slug.clone();
        move |txn| {
            Box::pin(async move {
                collections::insert(
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
                .await
            })
        }
    })
    .await
    .map_err(|error| map_create_conflict(error, &request_id))?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(collection_response(collection, request_id)),
    ))
}

async fn update_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(collection_id): AppPath<Uuid>,
    AppJson(body): AppJson<UpdateBody>,
) -> Result<Json<CollectionResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    require_perm(&auth.context, COLLECTION_WRITE_PERM, &request_id)?;
    require_coll(&auth.context, collection_id, &request_id)
        .map_err(|_| deny_or_not_found(&request_id))?;
    if body.name.is_none() && body.description.is_none() && body.visibility.is_none() {
        return Err(ApiRejection::validation(
            "at least one field is required",
            &request_id,
        ));
    }
    if let Some(ref name) = body.name {
        if name.trim().is_empty() || name.len() > 200 {
            return Err(ApiRejection::validation(
                "name must be 1..=200 characters",
                &request_id,
            ));
        }
    }
    let visibility = match body.visibility.as_deref() {
        None => None,
        Some("org") => Some(CollectionVisibility::Org),
        Some("private") => Some(CollectionVisibility::Private),
        Some("groups") => Some(CollectionVisibility::Groups),
        Some(_) => {
            return Err(ApiRejection::validation(
                "visibility must be one of org|private|groups",
                &request_id,
            ))
        }
    };
    let name = body.name.as_deref().map(str::trim).map(str::to_string);
    let description = body.description.map(|value| {
        value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    });
    let actor = auth.context.user_id();
    let collection = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let name = name.clone();
        let description = description.clone();
        move |txn| {
            Box::pin(async move {
                let existing = collections::get_by_id(txn, &ctx, collection_id).await?;
                // Ownership or existing allow-list scope (checked above) is required;
                // non-owners still need write permission + allow-list membership.
                if existing.owner_user_id != actor && !ctx.allows_collection(collection_id) {
                    return Err(crate::db::error::DbError::NotFound);
                }
                collections::update(
                    txn,
                    &ctx,
                    collection_id,
                    UpdateCollection {
                        name: name.as_deref(),
                        description: description.as_ref().map(|value| value.as_deref()),
                        visibility,
                    },
                )
                .await
            })
        }
    })
    .await
    .map_err(|error| map_create_conflict(error, &request_id))?;
    Ok(Json(collection_response(collection, request_id)))
}

fn map_create_conflict(error: crate::db::error::DbError, request_id: &str) -> ApiRejection {
    match &error {
        crate::db::error::DbError::Query(pg)
            if pg
                .code()
                .is_some_and(|code| code == &tokio_postgres::error::SqlState::UNIQUE_VIOLATION) =>
        {
            ApiRejection::conflict(
                "collection_conflict",
                "Collection name or slug already exists",
                request_id,
            )
        }
        _ => map_db(error, request_id),
    }
}
