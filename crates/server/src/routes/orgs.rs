//! Organization lifecycle routes (1C-01, lifecycle half): list/detail/switch.
//!
//! Deliberately NOT `AuthenticatedOrg` for any of these three endpoints: that
//! extractor resolves `OrgContext` from the *JWT's* `org_id` claim, which is
//! the wrong tool here on purpose — list/detail/switch exist so a caller
//! whose bearer token is scoped to org A can discover, inspect, or move to
//! org B, and a caller whose org-A membership was just revoked must still be
//! able to list or switch into whatever other orgs they belong to. Instead,
//! authorization here is: a cryptographically valid, unexpired bearer access
//! token (proves the caller's `user_id` only — same "auth-only" trust level
//! `members::accept_invite` already uses for the same reason) plus a fresh
//! PostgreSQL membership re-check against whatever target `org_id` the
//! request names. The JWT `org_id` claim never gates any of these three
//! routes; it is not even read.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::{ApiError, Page, PageInfo};
use crate::auth::middleware::{session_error_response, verify_bearer_claims};
use crate::auth::permissions::ResolveError;
use crate::auth::session::switch_org;
use crate::db::models::MembershipRole;
use crate::http::AppState;
use crate::middleware::RequestId;
use crate::routes::auth::TokenResponse;
use crate::services::orgs::{self, OrgSummary};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/orgs", get(list_orgs))
        .route("/api/v1/orgs/switch", post(switch))
        .route("/api/v1/orgs/{org_id}", get(get_org))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrgDto {
    id: Uuid,
    slug: String,
    name: String,
    role: &'static str,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchOrgRequest {
    org_id: Uuid,
}

fn org_dto(org: crate::db::models::Org, role: MembershipRole) -> OrgDto {
    OrgDto {
        id: org.id,
        slug: org.slug,
        name: org.name,
        role: role.as_str(),
        created_at: org.created_at,
    }
}

fn summary_dto(summary: OrgSummary) -> OrgDto {
    org_dto(summary.org, summary.role)
}

fn request_id_from_ext(ext: Option<Extension<RequestId>>) -> String {
    ext.map(|id| id.0 .0)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

/// Verifies the bearer access token and returns the caller's `user_id`.
///
/// Signature/issuer/audience/kid/exp/nbf are all checked by
/// [`verify_bearer_claims`] — this never trusts the token's `org_id` claim,
/// which is exactly the point of these three routes (see module doc).
fn authenticate(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    request_id: &str,
) -> Result<Uuid, RouteError> {
    let Some(provider) = state.auth_provider() else {
        return Err(RouteError::Unavailable(request_id.to_string()));
    };
    let Some(authorization) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(RouteError::Unauthorized(request_id.to_string()));
    };
    let claims = verify_bearer_claims(provider.keys(), authorization)
        .map_err(|_| RouteError::Unauthorized(request_id.to_string()))?;
    Uuid::parse_str(&claims.sub).map_err(|_| RouteError::Unauthorized(request_id.to_string()))
}

// ---------------------------------------------------------------------
// GET /orgs — only orgs the caller is currently an active member of.
// ---------------------------------------------------------------------

async fn list_orgs(
    State(state): State<Arc<AppState>>,
    request_id_ext: Option<Extension<RequestId>>,
    headers: HeaderMap,
) -> Result<Json<Page<OrgDto>>, RouteError> {
    let request_id = request_id_from_ext(request_id_ext);
    let user_id = authenticate(&state, &headers, &request_id)?;
    let summaries = orgs::list_user_orgs(state.pool(), user_id)
        .await
        .map_err(|error| RouteError::from_resolve(error, &request_id))?;
    // Unpaginated by design: the result is bounded by how many orgs one user
    // can belong to, the same shape `/members`/`/members/invites` already
    // use for their own bounded, non-paginated lists.
    Ok(Json(Page {
        items: summaries.into_iter().map(summary_dto).collect(),
        page: PageInfo {
            next_cursor: None,
            has_more: false,
        },
    }))
}

// ---------------------------------------------------------------------
// GET /orgs/{orgId} — detail, only if caller is an active member.
// ---------------------------------------------------------------------

async fn get_org(
    State(state): State<Arc<AppState>>,
    request_id_ext: Option<Extension<RequestId>>,
    headers: HeaderMap,
    Path(org_id): Path<Uuid>,
) -> Result<Json<OrgDto>, RouteError> {
    let request_id = request_id_from_ext(request_id_ext);
    let user_id = authenticate(&state, &headers, &request_id)?;
    let detail = orgs::get_org_detail(state.pool(), user_id, org_id)
        .await
        .map_err(|error| RouteError::from_resolve(error, &request_id))?;
    match detail {
        Some(detail) => Ok(Json(org_dto(detail.org, detail.role))),
        // Same response for "no such org" and "not a member" — see module doc.
        None => Err(RouteError::NotFound(request_id)),
    }
}

// ---------------------------------------------------------------------
// POST /orgs/switch — re-verify membership, mint a fresh session, audit.
// ---------------------------------------------------------------------

async fn switch(
    State(state): State<Arc<AppState>>,
    request_id_ext: Option<Extension<RequestId>>,
    client_ip: Option<Extension<crate::middleware::ClientIp>>,
    headers: HeaderMap,
    Json(body): Json<SwitchOrgRequest>,
) -> Response {
    let request_id = request_id_from_ext(request_id_ext);
    let ip = client_ip
        .map(|ext| ext.0 .0.clone())
        .unwrap_or_else(|| "unknown".into());
    // Token-minting endpoint, same abuse surface as /auth/login and
    // /auth/refresh — apply the same IP-scoped auth limiter.
    if let Err(rejected) = crate::routes::rate_limit_guard::check_auth_ip(&state, &ip, &request_id)
    {
        return rejected.into_response();
    }
    let user_id = match authenticate(&state, &headers, &request_id) {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };
    let Some(provider) = state.auth_provider() else {
        return session_error_response(
            crate::auth::session::SessionError::NotConfigured,
            &request_id,
        );
    };
    match switch_org(
        provider.pool(),
        provider.auth_config(),
        provider.keys(),
        user_id,
        body.org_id,
        &request_id,
    )
    .await
    {
        Ok(tokens) => Json(TokenResponse::from(tokens)).into_response(),
        Err(error) => session_error_response(error, &request_id),
    }
}

// ---------------------------------------------------------------------
// Error mapping (list/detail only — switch reuses `session_error_response`).
// ---------------------------------------------------------------------

enum RouteError {
    Unauthorized(String),
    Unavailable(String),
    NotFound(String),
    Database(String),
}

impl RouteError {
    fn from_resolve(error: ResolveError, request_id: &str) -> Self {
        // `list_user_orgs`/`get_org_detail` already fold every authorization
        // outcome (missing org, missing/suspended membership, disabled user)
        // into `Ok`-side skip/`None`; whatever reaches here is a genuine
        // infrastructure failure, not a denial.
        let _ = error;
        Self::Database(request_id.to_string())
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        let (status, code, message, request_id) = match self {
            Self::Unauthorized(request_id) => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Missing or invalid bearer token",
                request_id,
            ),
            Self::Unavailable(request_id) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Authentication is not configured",
                request_id,
            ),
            Self::NotFound(request_id) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Organization not found",
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
