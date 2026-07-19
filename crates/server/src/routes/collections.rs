//! Collection REST routes.

use std::sync::Arc;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::PageInfo;
use crate::auth::middleware::AuthenticatedOrg;
use crate::db::collections::{self, NewCollection};
use crate::db::models::{Collection, CollectionVisibility};
use crate::db::pool::with_org_txn_typed;
use crate::http::AppState;
use crate::routes::common::{
    decode_cursor, encode_cursor, page_info, parse_page_limit, parse_uuid,
    require_collection_or_404, require_permission_or_403, ListResponse, PageParams, RestError,
    TxnRestError,
};

const JSON_BODY_LIMIT: usize = 16 * 1024;
const COLLECTION_CURSOR: &str = "collection.name_id.v1";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/collections",
            get(list_collections).post(create_collection),
        )
        .route("/api/v1/collections/{collectionId}", get(get_collection))
        .route_layer(DefaultBodyLimit::max(JSON_BODY_LIMIT))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionPath {
    collection_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectionCursor {
    name: String,
    id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCollectionRequest {
    name: String,
    slug: String,
    description: Option<String>,
    visibility: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectionResponse {
    id: Uuid,
    name: String,
    slug: String,
    description: Option<String>,
    owner_user_id: Uuid,
    visibility: &'static str,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<Collection> for CollectionResponse {
    fn from(value: Collection) -> Self {
        Self {
            id: value.id,
            name: value.name,
            slug: value.slug,
            description: value.description,
            owner_user_id: value.owner_user_id,
            visibility: value.visibility.as_str(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

async fn list_collections(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    query: Result<Query<PageParams>, QueryRejection>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let Query(params) =
        query.map_err(|_| RestError::validation("query string is invalid", &request_id))?;
    let page_limit = parse_page_limit(&params, &request_id)?;
    let cursor_key = state
        .download_capability_key()
        .ok_or_else(|| RestError::service_unavailable(&request_id))?;
    let after: Option<CollectionCursor> = decode_cursor(
        cursor_key,
        COLLECTION_CURSOR,
        params.cursor.as_deref(),
        &request_id,
    )?;
    let allowed: Vec<Uuid> = auth
        .context
        .allowed_collection_ids()
        .iter()
        .copied()
        .collect();

    let result = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| {
            Box::pin(async move {
                let after = after.map(|cursor| (cursor.name, cursor.id));
                let collections =
                    collections::list_authorized(txn, &ctx, &allowed, after, page_limit.fetch_size)
                        .await?;
                Ok::<_, TxnRestError>(collections)
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;

    let (items, page_info) = collection_page(cursor_key, result, page_limit.page_size)?;
    Ok(Json(ListResponse { items, page_info }).into_response())
}

async fn create_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    body: Result<Json<CreateCollectionRequest>, JsonRejection>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let Json(body) =
        body.map_err(|_| RestError::validation("request body is invalid", &request_id))?;
    let input = validate_create_collection(body, &request_id)?;
    let collection = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_permission_or_403(&ctx, "doc.upload", &request_id)?;
                let collection = collections::insert(
                    txn,
                    &ctx,
                    NewCollection {
                        id: Uuid::new_v4(),
                        name: &input.name,
                        slug: &input.slug,
                        description: input.description.as_deref(),
                        visibility: input.visibility,
                    },
                )
                .await?;
                Ok::<_, TxnRestError>(collection)
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;
    Ok((
        StatusCode::CREATED,
        Json(CollectionResponse::from(collection)),
    )
        .into_response())
}

async fn get_collection(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(path): Path<CollectionPath>,
) -> Result<Response, RestError> {
    let request_id = auth.request_id.clone();
    let collection_id = parse_uuid(&path.collection_id, &request_id)?;
    let collection = with_org_txn_typed(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        let request_id = request_id.clone();
        move |txn| {
            Box::pin(async move {
                require_collection_or_404(&ctx, collection_id, &request_id)?;
                let collection = collections::get_by_id(txn, &ctx, collection_id).await?;
                Ok::<_, TxnRestError>(collection)
            })
        }
    })
    .await
    .map_err(|error: TxnRestError| error.into_rest(&request_id))?;
    Ok(Json(CollectionResponse::from(collection)).into_response())
}

struct ValidCreateCollection {
    name: String,
    slug: String,
    description: Option<String>,
    visibility: CollectionVisibility,
}

fn validate_create_collection(
    body: CreateCollectionRequest,
    request_id: &str,
) -> Result<ValidCreateCollection, RestError> {
    let name = body.name.trim().to_string();
    if name.is_empty() || name.len() > 255 {
        return Err(RestError::validation(
            "name must be between 1 and 255 bytes",
            request_id,
        ));
    }
    let slug = body.slug.trim().to_string();
    if !valid_slug(&slug) {
        return Err(RestError::validation("slug is invalid", request_id));
    }
    let description = body
        .description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if description.as_ref().is_some_and(|value| value.len() > 4096) {
        return Err(RestError::validation("description is too long", request_id));
    }
    let visibility = match body.visibility.as_str() {
        "private" => CollectionVisibility::Private,
        "org" => CollectionVisibility::Org,
        _ => {
            return Err(RestError::validation(
                "visibility must be private or org",
                request_id,
            ));
        }
    };
    Ok(ValidCreateCollection {
        name,
        slug,
        description,
        visibility,
    })
}

fn valid_slug(value: &str) -> bool {
    let len = value.len();
    (2..=63).contains(&len)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn collection_page(
    cursor_key: &crate::services::download::CapabilityKey,
    mut rows: Vec<Collection>,
    page_size: usize,
) -> Result<(Vec<CollectionResponse>, PageInfo), RestError> {
    let has_more = rows.len() > page_size;
    if has_more {
        rows.truncate(page_size);
    }
    let next_cursor = if has_more {
        rows.last()
            .map(|item| {
                encode_cursor(
                    cursor_key,
                    COLLECTION_CURSOR,
                    &CollectionCursor {
                        name: item.name.clone(),
                        id: item.id,
                    },
                )
            })
            .transpose()?
    } else {
        None
    };
    let items = rows.into_iter().map(CollectionResponse::from).collect();
    Ok((items, page_info(next_cursor)))
}
