//! Membership + invite + usage admin routes (P2-11 / P2-12, Wave 2 surface).
//!
//! Builds the HTTP surface on top of the frozen Wave 1 domain layer
//! (`services::members`, `db::members`) — see
//! `plans/reports/plan-260728-0231-markhand-web-membership-admin-slice.md`
//! section 4 (E1-E8) and section 5 (per-mutation obligations). This module
//! must not change domain invariants; it only guards, maps errors to status
//! codes, and wires audit/session-revoke calls the domain layer documents as
//! Wave 2's responsibility.
//!
//! ## Known Wave 1 gap this route layer works around (read before editing)
//!
//! `services::members::{change_role, remove_member, suspend_member,
//! reactivate_member}` all call `db::members::get(txn, ctx, target_user_id)`
//! with a bare `?`, and `MemberError`'s blanket `From<DbError>` collapses
//! *every* `DbError` variant — including `DbError::NotFound` — to
//! `MemberError::Database`. That means a target user with no membership row
//! (the exact shape of a cross-org DELETE/PATCH, since RLS simply hides the
//! foreign row) would surface as a 500 instead of a 404 if this route called
//! those functions directly. `revoke_invite` in the same file *does*
//! special-case `DbError::NotFound` before this point — the other four
//! functions do not. This is a domain-layer bug (frozen file, out of Wave 2's
//! ownership — reported, not patched here). [`fetch_current_membership`]
//! below pre-checks existence through `db::members::get` directly (read-only,
//! same tenant-scoped transaction pattern) so PATCH/DELETE return a clean 404
//! before ever reaching the buggy path. This does not close a race against a
//! membership row deleted between the pre-check and the mutation transaction
//! (that residual case would still surface as 500) — see the Wave 2 report.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::{ApiError, Page, PageInfo};
use crate::auth::context::OrgContext;
use crate::auth::middleware::{verify_bearer_claims, AuthenticatedOrg};
use crate::auth::permissions::require_permission;
use crate::auth::session::revoke_all_user_families;
use crate::db::error::DbError;
use crate::db::members as member_rows;
use crate::db::models::{MembershipRole, MembershipState, OrgInvite, OrgMembership};
use crate::db::pool::with_org_txn;
use crate::http::AppState;
use crate::middleware::RequestId;
use crate::services::audit::{self, AuditAction};
use crate::services::members::{self, MemberError, ResourceUsage};

/// Permission code guarding every endpoint in this module except accept-invite
/// (auth-only by design — see the module-level accept-invite note in
/// `services::members` and the Wave 2 plan report section on the auth
/// wrinkle). Seeded in `migrations/0011` but unused before this slice.
const PERMISSION_MEMBER_MANAGE: &str = "member.manage";

const MAX_EMAIL_LEN: usize = 320;
const MAX_TOKEN_LEN: usize = 512;
const MIN_INVITE_TTL_SECS: i64 = 60;
const MAX_INVITE_TTL_SECS: i64 = 30 * 24 * 3600;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/members", get(list_members))
        .route(
            "/api/v1/members/invites",
            get(list_invites).post(create_invite),
        )
        .route("/api/v1/members/invites/accept", post(accept_invite))
        .route(
            "/api/v1/members/invites/{invite_id}/revoke",
            post(revoke_invite),
        )
        .route(
            "/api/v1/members/{user_id}",
            patch(patch_member).delete(delete_member),
        )
        .route("/api/v1/usage", get(usage_overview))
}

// ---------------------------------------------------------------------
// Wire DTOs (route-local — see ownership note: api/types.rs is not in this
// wave's edit list, so response shapes live here rather than being exported
// for reuse elsewhere).
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MembershipDto {
    user_id: Uuid,
    role: String,
    state: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteDto {
    id: Uuid,
    email: String,
    role: String,
    /// One of `pending`, `accepted`, `revoked`, `expired` — derived, not stored.
    status: String,
    expires_at: DateTime<Utc>,
    accepted_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteResponse {
    invite: InviteDto,
    /// Plaintext invite token, surfaced exactly once. Never logged, never
    /// written to audit metadata (see `services::members::CreatedInvite` doc
    /// and plan section 2 caveat C3).
    token: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageEntryDto {
    resource: String,
    limit: i64,
    committed: i64,
    reserved: i64,
    remaining: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageResponse {
    items: Vec<UsageEntryDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateInviteRequest {
    email: String,
    role: String,
    #[serde(default)]
    ttl_secs: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcceptInviteRequest {
    token: String,
}

/// Hand-written (never derived) so a stray `{:?}` on this request never
/// prints the plaintext invite token.
impl std::fmt::Debug for AcceptInviteRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcceptInviteRequest")
            .field("token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PatchMemberRequest {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

fn membership_dto(row: OrgMembership) -> MembershipDto {
    MembershipDto {
        user_id: row.user_id,
        role: row.role.as_str().to_string(),
        state: row.state.as_str().to_string(),
        created_at: row.created_at,
    }
}

fn invite_status(invite: &OrgInvite, now: DateTime<Utc>) -> &'static str {
    if invite.revoked_at.is_some() {
        "revoked"
    } else if invite.accepted_at.is_some() {
        "accepted"
    } else if invite.expires_at <= now {
        "expired"
    } else {
        "pending"
    }
}

fn invite_dto(invite: &OrgInvite) -> InviteDto {
    let now = Utc::now();
    InviteDto {
        id: invite.id,
        email: invite.email.clone(),
        role: invite.role.as_str().to_string(),
        status: invite_status(invite, now).to_string(),
        expires_at: invite.expires_at,
        accepted_at: invite.accepted_at,
        revoked_at: invite.revoked_at,
        created_at: invite.created_at,
    }
}

fn usage_dto(usage: ResourceUsage) -> UsageEntryDto {
    UsageEntryDto {
        resource: usage.kind.as_str().to_string(),
        limit: usage.limit,
        committed: usage.committed,
        reserved: usage.reserved,
        remaining: usage.remaining,
    }
}

/// Ranks roles for downgrade detection (`change_role` obligation: revoke
/// refresh-token families after a role *downgrade*, not an upgrade). Higher
/// is more privileged.
fn role_rank(role: MembershipRole) -> u8 {
    match role {
        MembershipRole::Owner => 3,
        MembershipRole::Admin => 2,
        MembershipRole::Editor => 1,
        MembershipRole::Viewer => 0,
    }
}

/// Read-only existence pre-check — see the module doc's Wave 1 gap note.
/// Returns `Ok(None)` for both "no such user" and "exists in another org"
/// (RLS makes the two indistinguishable, which is the point).
async fn fetch_current_membership(
    pool: &Pool,
    ctx: &OrgContext,
    user_id: Uuid,
) -> Result<Option<OrgMembership>, DbError> {
    with_org_txn(pool, ctx, {
        let ctx = ctx.clone();
        move |txn| {
            Box::pin(async move {
                match member_rows::get(txn, &ctx, user_id).await {
                    Ok(row) => Ok(Some(row)),
                    Err(DbError::NotFound) => Ok(None),
                    Err(other) => Err(other),
                }
            })
        }
    })
    .await
}

fn request_id_from_ext(ext: Option<Extension<RequestId>>) -> String {
    ext.map(|id| id.0 .0)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

// ---------------------------------------------------------------------
// E1 — GET /members
// ---------------------------------------------------------------------

async fn list_members(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
) -> Result<Json<Page<MembershipDto>>, RouteError> {
    require_permission(&auth.context, PERMISSION_MEMBER_MANAGE)
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let items = members::list_members(state.pool(), &auth.context)
        .await
        .map_err(|error| RouteError::from_member(error, &auth.request_id))?;
    Ok(Json(Page {
        items: items.into_iter().map(membership_dto).collect(),
        page: PageInfo {
            next_cursor: None,
            has_more: false,
        },
    }))
}

// ---------------------------------------------------------------------
// E2 — GET /members/invites
// ---------------------------------------------------------------------

async fn list_invites(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
) -> Result<Json<Page<InviteDto>>, RouteError> {
    require_permission(&auth.context, PERMISSION_MEMBER_MANAGE)
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let items = members::list_invites(state.pool(), &auth.context)
        .await
        .map_err(|error| RouteError::from_member(error, &auth.request_id))?;
    Ok(Json(Page {
        items: items.iter().map(invite_dto).collect(),
        page: PageInfo {
            next_cursor: None,
            has_more: false,
        },
    }))
}

// ---------------------------------------------------------------------
// E3 — POST /members/invites
// ---------------------------------------------------------------------

async fn create_invite(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Json(body): Json<CreateInviteRequest>,
) -> Result<(StatusCode, Json<CreateInviteResponse>), RouteError> {
    if require_permission(&auth.context, PERMISSION_MEMBER_MANAGE).is_err() {
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            AuditAction::MemberInvite.as_str(),
            "member",
            None,
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }

    let email = body.email.trim();
    if email.is_empty() || email.len() > MAX_EMAIL_LEN || !email.contains('@') {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid email",
        ));
    }
    let role = MembershipRole::parse(&body.role)
        .map_err(|_| RouteError::Validation(auth.request_id.clone(), "Invalid role"))?;
    let ttl_secs = body.ttl_secs.unwrap_or(members::DEFAULT_INVITE_TTL_SECS);
    if !(MIN_INVITE_TTL_SECS..=MAX_INVITE_TTL_SECS).contains(&ttl_secs) {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "Invalid ttlSecs",
        ));
    }

    let created = members::create_invite(
        state.pool(),
        &auth.context,
        &auth.request_id,
        email,
        role,
        ttl_secs,
    )
    .await
    .map_err(|error| RouteError::from_member(error, &auth.request_id))?;

    Ok((
        StatusCode::CREATED,
        Json(CreateInviteResponse {
            invite: invite_dto(&created.invite),
            token: created.plaintext_token.expose().to_string(),
        }),
    ))
}

// ---------------------------------------------------------------------
// E4 — POST /members/invites/{inviteId}/revoke
// ---------------------------------------------------------------------

async fn revoke_invite(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(invite_id): Path<Uuid>,
) -> Result<Json<InviteDto>, RouteError> {
    if require_permission(&auth.context, PERMISSION_MEMBER_MANAGE).is_err() {
        let resource_id = invite_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            AuditAction::MemberInviteRevoke.as_str(),
            "member",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }

    let invite = members::revoke_invite(state.pool(), &auth.context, &auth.request_id, invite_id)
        .await
        .map_err(|error| RouteError::from_member(error, &auth.request_id))?;
    Ok(Json(invite_dto(&invite)))
}

// ---------------------------------------------------------------------
// E5 — POST /members/invites/accept (auth-only; see module + report notes)
// ---------------------------------------------------------------------

async fn accept_invite(
    State(state): State<Arc<AppState>>,
    request_id_ext: Option<Extension<RequestId>>,
    headers: HeaderMap,
    Json(body): Json<AcceptInviteRequest>,
) -> Result<(StatusCode, Json<MembershipDto>), RouteError> {
    let request_id = request_id_from_ext(request_id_ext);

    let Some(provider) = state.auth_provider() else {
        return Err(RouteError::Unavailable(request_id));
    };

    // Deliberately NOT `AuthenticatedOrg`: that extractor resolves org
    // membership from current PG state and would 403 a caller who is not yet
    // a member of the invite's target org — exactly the caller this endpoint
    // exists for. Authorization here is: (a) a cryptographically valid,
    // unexpired bearer access token (proves the caller is *some*
    // authenticated principal — `sub` is the accepting user id) plus (b) the
    // invite token itself (proves the bearer was invited into a specific
    // org). Neither check touches org membership. See the Wave 2 report for
    // the one case this does NOT cover (a brand new user with zero org
    // memberships anywhere cannot obtain a bearer token at all today, because
    // `auth::session::login_with_password` requires an existing membership to
    // mint one — that gap is outside this route's ownership to fix).
    let Some(authorization) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(RouteError::Unauthorized(request_id));
    };
    let claims = verify_bearer_claims(provider.keys(), authorization)
        .map_err(|_| RouteError::Unauthorized(request_id.clone()))?;
    let user_id =
        Uuid::parse_str(&claims.sub).map_err(|_| RouteError::Unauthorized(request_id.clone()))?;

    let token = body.token.trim();
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return Err(RouteError::Validation(request_id, "Invalid token"));
    }

    let membership = members::accept_invite(state.pool(), token, user_id, &request_id)
        .await
        .map_err(|error| RouteError::from_member(error, &request_id))?;
    Ok((StatusCode::CREATED, Json(membership_dto(membership))))
}

// ---------------------------------------------------------------------
// E6 — PATCH /members/{userId}
// ---------------------------------------------------------------------

async fn patch_member(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(user_id): Path<Uuid>,
    Json(body): Json<PatchMemberRequest>,
) -> Result<Json<MembershipDto>, RouteError> {
    if require_permission(&auth.context, PERMISSION_MEMBER_MANAGE).is_err() {
        let resource_id = user_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            AuditAction::MemberRoleChange.as_str(),
            "member",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }

    let new_role = match &body.role {
        Some(raw) => Some(
            MembershipRole::parse(raw)
                .map_err(|_| RouteError::Validation(auth.request_id.clone(), "Invalid role"))?,
        ),
        None => None,
    };
    let new_state = match &body.state {
        Some(raw) => Some(
            MembershipState::parse(raw)
                .map_err(|_| RouteError::Validation(auth.request_id.clone(), "Invalid state"))?,
        ),
        None => None,
    };
    if new_role.is_none() && new_state.is_none() {
        return Err(RouteError::Validation(
            auth.request_id.clone(),
            "role or state is required",
        ));
    }

    // Pre-check existence to avoid the Wave 1 NotFound->Database masking bug
    // documented at the top of this file (also the key mechanism by which a
    // cross-org PATCH surfaces as 404 rather than 500/403).
    let Some(existing) = fetch_current_membership(state.pool(), &auth.context, user_id)
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?
    else {
        return Err(RouteError::NotFound(auth.request_id.clone()));
    };

    let mut current = existing;
    let mut should_revoke = false;

    if let Some(role) = new_role {
        if role_rank(role) < role_rank(current.role) {
            should_revoke = true;
        }
        current =
            members::change_role(state.pool(), &auth.context, &auth.request_id, user_id, role)
                .await
                .map_err(|error| RouteError::from_member(error, &auth.request_id))?;
    }

    if let Some(target_state) = new_state {
        current = match target_state {
            MembershipState::Suspended => {
                should_revoke = true;
                members::suspend_member(state.pool(), &auth.context, &auth.request_id, user_id)
                    .await
                    .map_err(|error| RouteError::from_member(error, &auth.request_id))?
            }
            MembershipState::Active => {
                members::reactivate_member(state.pool(), &auth.context, &auth.request_id, user_id)
                    .await
                    .map_err(|error| RouteError::from_member(error, &auth.request_id))?
            }
        };
    }

    if should_revoke {
        revoke_all_user_families(
            state.pool(),
            auth.context.org_id(),
            user_id,
            &auth.request_id,
            "member_downgraded_or_suspended",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    }

    Ok(Json(membership_dto(current)))
}

// ---------------------------------------------------------------------
// E7 — DELETE /members/{userId}
// ---------------------------------------------------------------------

async fn delete_member(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, RouteError> {
    if require_permission(&auth.context, PERMISSION_MEMBER_MANAGE).is_err() {
        let resource_id = user_id.to_string();
        audit::record_deny(
            state.pool(),
            &auth.context,
            &auth.request_id,
            AuditAction::MemberRemove.as_str(),
            "member",
            Some(&resource_id),
            "permission_denied",
        )
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
        return Err(RouteError::Denied(auth.request_id.clone()));
    }

    // Same pre-check rationale as `patch_member` above.
    let existing = fetch_current_membership(state.pool(), &auth.context, user_id)
        .await
        .map_err(|_| RouteError::Database(auth.request_id.clone()))?;
    if existing.is_none() {
        return Err(RouteError::NotFound(auth.request_id.clone()));
    }

    members::remove_member(state.pool(), &auth.context, &auth.request_id, user_id)
        .await
        .map_err(|error| RouteError::from_member(error, &auth.request_id))?;

    // Required by plan section 5 / services::members doc even though
    // `remove_member` already hard-deletes refresh_tokens in the same
    // transaction (this call becomes a harmless no-op in that case).
    revoke_all_user_families(
        state.pool(),
        auth.context.org_id(),
        user_id,
        &auth.request_id,
        "member_removed",
    )
    .await
    .map_err(|_| RouteError::Database(auth.request_id.clone()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------
// E8 — GET /usage
// ---------------------------------------------------------------------

async fn usage_overview(
    State(state): State<Arc<AppState>>,
    auth: AuthenticatedOrg,
) -> Result<Json<UsageResponse>, RouteError> {
    require_permission(&auth.context, PERMISSION_MEMBER_MANAGE)
        .map_err(|_| RouteError::Denied(auth.request_id.clone()))?;
    let items = members::usage_overview(state.pool(), &auth.context)
        .await
        .map_err(|error| RouteError::from_member(error, &auth.request_id))?;
    Ok(Json(UsageResponse {
        items: items.into_iter().map(usage_dto).collect(),
    }))
}

// ---------------------------------------------------------------------
// Error mapping (fail-closed; see plan section on cross-org denial)
// ---------------------------------------------------------------------

enum RouteError {
    Denied(String),
    Validation(String, &'static str),
    NotFound(String),
    /// (request_id, machine code, human message)
    Conflict(String, &'static str, &'static str),
    Unauthorized(String),
    Unavailable(String),
    Database(String),
}

impl RouteError {
    fn from_member(error: MemberError, request_id: &str) -> Self {
        let request_id = request_id.to_string();
        match error {
            MemberError::NotFound | MemberError::InviteNotFound => Self::NotFound(request_id),
            MemberError::InvalidToken => Self::Validation(request_id, "Invite token is malformed"),
            MemberError::InviteTerminal => Self::Conflict(
                request_id,
                "invite_terminal",
                "Invite has already been accepted or revoked",
            ),
            MemberError::InviteExpired => {
                Self::Conflict(request_id, "invite_expired", "Invite has expired")
            }
            MemberError::AlreadyMember => Self::Conflict(
                request_id,
                "already_member",
                "User is already a member of this organization",
            ),
            MemberError::LastOwner => Self::Conflict(
                request_id,
                "last_owner",
                "Operation would leave the organization with zero active owners",
            ),
            MemberError::OwnerRequiredForOwnerInvite | MemberError::OwnerRequiredToManageOwner => {
                Self::Denied(request_id)
            }
            MemberError::Database => Self::Database(request_id),
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
                "Member resource not found",
                request_id,
            ),
            Self::Conflict(request_id, code, message) => {
                (StatusCode::CONFLICT, code, message, request_id)
            }
            Self::Unauthorized(request_id) => (
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Authentication required",
                request_id,
            ),
            Self::Unavailable(request_id) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Authentication is not configured",
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

    fn status_of(error: MemberError) -> StatusCode {
        RouteError::from_member(error, "11111111-1111-1111-1111-111111111111")
            .into_response()
            .status()
    }

    #[test]
    fn member_error_status_mapping_matches_plan_section_4() {
        assert_eq!(status_of(MemberError::NotFound), StatusCode::NOT_FOUND);
        assert_eq!(
            status_of(MemberError::InviteNotFound),
            StatusCode::NOT_FOUND
        );
        assert_eq!(status_of(MemberError::LastOwner), StatusCode::CONFLICT);
        assert_eq!(status_of(MemberError::InviteExpired), StatusCode::CONFLICT);
        assert_eq!(status_of(MemberError::InviteTerminal), StatusCode::CONFLICT);
        assert_eq!(status_of(MemberError::AlreadyMember), StatusCode::CONFLICT);
        assert_eq!(
            status_of(MemberError::OwnerRequiredForOwnerInvite),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(MemberError::OwnerRequiredToManageOwner),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(MemberError::InvalidToken),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(MemberError::Database),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn conflict_codes_are_distinguishable_machine_codes() {
        let codes: Vec<&str> = [
            MemberError::LastOwner,
            MemberError::InviteExpired,
            MemberError::InviteTerminal,
            MemberError::AlreadyMember,
        ]
        .into_iter()
        .map(|error| match RouteError::from_member(error, "req") {
            RouteError::Conflict(_, code, _) => code,
            _ => unreachable!(),
        })
        .collect();
        let unique: std::collections::BTreeSet<&str> = codes.iter().copied().collect();
        assert_eq!(unique.len(), codes.len(), "conflict codes must be unique");
    }

    #[test]
    fn role_rank_orders_owner_above_viewer() {
        assert!(role_rank(MembershipRole::Owner) > role_rank(MembershipRole::Admin));
        assert!(role_rank(MembershipRole::Admin) > role_rank(MembershipRole::Editor));
        assert!(role_rank(MembershipRole::Editor) > role_rank(MembershipRole::Viewer));
    }

    #[test]
    fn invite_status_reflects_terminal_and_expiry_precedence() {
        use chrono::TimeZone;
        let base = OrgInvite {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            email: "invitee@example.com".into(),
            role: MembershipRole::Editor,
            token_hash: crate::db::models::SecretHash::new("deadbeef".repeat(8)),
            invited_by_user_id: Uuid::new_v4(),
            expires_at: Utc.with_ymd_and_hms(2100, 1, 1, 0, 0, 0).unwrap(),
            accepted_at: None,
            revoked_at: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        };
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        assert_eq!(invite_status(&base, now), "pending");

        let mut expired = base.clone();
        expired.expires_at = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(invite_status(&expired, now), "expired");

        let mut accepted = base.clone();
        accepted.accepted_at = Some(now);
        assert_eq!(invite_status(&accepted, now), "accepted");

        // Revoked takes precedence even if also (hypothetically) accepted.
        let mut revoked = base.clone();
        revoked.revoked_at = Some(now);
        assert_eq!(invite_status(&revoked, now), "revoked");
    }
}
