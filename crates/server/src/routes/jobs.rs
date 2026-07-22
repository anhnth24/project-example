//! Job status REST routes (`/api/v1/jobs`).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::{
    decode_cursor, encode_cursor, ApiRejection, AppPath, AppQuery, CreatedAtIdCursor, JobResponse,
    ListResponse, PageInfo, PageParams,
};
use crate::auth::middleware::AuthenticatedOrg;
use crate::db::jobs as jobs_repo;
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::routes::common::{deny_or_not_found, job_response, load_document_authorized, map_db};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/jobs", get(list_jobs))
        .route("/api/v1/jobs/{job_id}", get(get_job))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    limit: Option<u32>,
    cursor: Option<String>,
    document_id: Option<Uuid>,
}

async fn list_jobs(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppQuery(query): AppQuery<ListQuery>,
) -> Result<Json<ListResponse<JobResponse>>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let page = PageParams::from_query(query.limit, query.cursor, &request_id)?;
    if let Some(document_id) = query.document_id {
        let _ = load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    }
    let after = match page.cursor.as_deref() {
        Some(raw) => Some(decode_cursor::<CreatedAtIdCursor>(raw).map_err(|message| {
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
        let document_id = query.document_id;
        let after_created_at = after.as_ref().map(|cursor| cursor.created_at);
        let after_id = after.as_ref().map(|cursor| cursor.id);
        move |txn| {
            Box::pin(async move {
                jobs_repo::list_page(
                    txn,
                    &ctx,
                    &allowed,
                    document_id,
                    fetch_limit,
                    after_created_at,
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
            encode_cursor(&CreatedAtIdCursor {
                created_at: row.created_at,
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
            .map(|row| job_response(row, request_id.clone()))
            .collect(),
        page_info,
        request_id,
    }))
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    AppPath(job_id): AppPath<Uuid>,
) -> Result<Json<JobResponse>, ApiRejection> {
    let request_id = auth.request_id.clone();
    let job = with_org_txn(state.pool(), &auth.context, {
        let ctx = auth.context.clone();
        move |txn| Box::pin(async move { jobs_repo::get_by_id(txn, &ctx, job_id).await })
    })
    .await
    .map_err(|error| map_db(error, &request_id))?
    .ok_or_else(|| deny_or_not_found(&request_id))?;
    if let Some(document_id) = job.document_id {
        let _ = load_document_authorized(&state, &auth.context, document_id, &request_id).await?;
    }
    Ok(Json(job_response(job, request_id)))
}
