//! Organization lifecycle routes (1C-01, full slice): create/list/detail/switch.
//!
//! Deliberately NOT `AuthenticatedOrg` for any of these four endpoints: that
//! extractor resolves `OrgContext` from the *JWT's* `org_id` claim, which is
//! the wrong tool here on purpose — list/detail/switch exist so a caller
//! whose bearer token is scoped to org A can discover, inspect, or move to
//! org B, and a caller whose org-A membership was just revoked must still be
//! able to list or switch into whatever other orgs they belong to; create
//! mints authority over a brand new org the caller has no membership in yet.
//! Instead, authorization here is: a cryptographically valid, unexpired
//! bearer access token (proves the caller's `user_id` only — same "auth-only"
//! trust level `members::accept_invite` already uses for the same reason)
//! plus, for list/detail/switch, a fresh PostgreSQL membership re-check
//! against whatever target `org_id` the request names. The JWT `org_id`
//! claim never gates any of these four routes; it is not even read.
//!
//! `create_org` (`POST /orgs`) is unblocked by 1C-03's global RBAC catalog
//! (`migrations/0030`) — see `services::orgs::create_org` for the
//! provisioning transaction.

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
use crate::services::orgs::{self, CreateOrgError, OrgSummary};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/orgs", get(list_orgs).post(create_org))
        .route("/api/v1/orgs/switch", post(switch))
        .route("/api/v1/orgs/{org_id}", get(get_org))
}

const MAX_SLUG_LEN: usize = 63;
const MAX_NAME_LEN: usize = 200;

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
struct CreateOrgRequest {
    slug: String,
    name: String,
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

/// Matches the `orgs.slug` CHECK constraint (migrations/0001) exactly:
/// `^[a-z0-9][a-z0-9-]{1,62}$` — 2-63 chars, lowercase alnum/hyphen, must not
/// start with a hyphen. Written by hand (no `regex` dependency in this
/// crate) rather than adding one for a single narrow pattern.
fn is_valid_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    if bytes.len() < 2 || bytes.len() > MAX_SLUG_LEN {
        return false;
    }
    let first_ok = matches!(bytes[0], b'a'..=b'z' | b'0'..=b'9');
    first_ok
        && bytes
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

/// Verifies the bearer access token and returns the caller's `user_id`.
///
/// Signature/issuer/audience/kid/exp/nbf are all checked by
/// [`verify_bearer_claims`] — this never trusts the token's `org_id` claim,
/// which is exactly the point of these routes (see module doc).
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
// POST /orgs — create a new org; caller becomes its owner.
// ---------------------------------------------------------------------

async fn create_org(
    State(state): State<Arc<AppState>>,
    request_id_ext: Option<Extension<RequestId>>,
    client_ip: Option<Extension<crate::middleware::ClientIp>>,
    headers: HeaderMap,
    Json(body): Json<CreateOrgRequest>,
) -> Response {
    let request_id = request_id_from_ext(request_id_ext);
    let ip = client_ip
        .map(|ext| ext.0 .0.clone())
        .unwrap_or_else(|| "unknown".into());

    // Token/session-adjacent mutation (mints owner authority over a brand
    // new org) — same IP-scoped auth limiter as /auth/login and the rest of
    // this backlog's org routes.
    if let Err(rejected) = crate::routes::rate_limit_guard::check_auth_ip(&state, &ip, &request_id)
    {
        return rejected.into_response();
    }

    let user_id = match authenticate(&state, &headers, &request_id) {
        Ok(user_id) => user_id,
        Err(error) => return error.into_response(),
    };

    let slug = body.slug.trim();
    let name = body.name.trim();
    if !is_valid_slug(slug) || name.is_empty() || name.len() > MAX_NAME_LEN {
        return RouteError::Validation(request_id).into_response();
    }

    match orgs::create_org(state.pool(), user_id, slug, name, &request_id).await {
        Ok(created) => (
            StatusCode::CREATED,
            Json(org_dto(created.org, created.role)),
        )
            .into_response(),
        Err(CreateOrgError::SlugTaken) => RouteError::SlugTaken(request_id).into_response(),
        Err(CreateOrgError::Database) => RouteError::Database(request_id).into_response(),
    }
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
// Error mapping (list/detail/create only — switch reuses
// `session_error_response`).
// ---------------------------------------------------------------------

enum RouteError {
    Unauthorized(String),
    Unavailable(String),
    Validation(String),
    SlugTaken(String),
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
            Self::Validation(request_id) => (
                StatusCode::BAD_REQUEST,
                "validation_failed",
                "Invalid org slug or name",
                request_id,
            ),
            Self::SlugTaken(request_id) => (
                StatusCode::CONFLICT,
                "slug_taken",
                "Organization slug is already taken",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validation_matches_the_orgs_table_check_constraint() {
        assert!(is_valid_slug("poc"));
        assert!(is_valid_slug("ab"));
        assert!(is_valid_slug("a1-b2"));
        assert!(!is_valid_slug("a"), "single char is too short");
        assert!(!is_valid_slug(""), "empty");
        assert!(!is_valid_slug("-abc"), "must not start with a hyphen");
        assert!(!is_valid_slug("Abc"), "must be lowercase");
        assert!(!is_valid_slug("ab_c"), "underscore not allowed");
        assert!(!is_valid_slug(&"a".repeat(64)), "over max length");
        assert!(is_valid_slug(&"a".repeat(63)), "exactly max length");
    }
}
