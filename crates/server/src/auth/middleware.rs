//! Axum extractors that resolve OrgContext from current PostgreSQL state.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use uuid::Uuid;

use crate::api::ApiError;
use crate::auth::context::OrgContext;
use crate::auth::jwt::{AccessClaims, JwtKeys};
use crate::auth::permissions::{resolve_org_context_in_txn, ResolveError};
use crate::auth::session::SessionError;

/// Authenticated request with OrgContext loaded from current PG membership.
#[derive(Debug, Clone)]
pub struct AuthenticatedOrg {
    pub context: OrgContext,
    pub claims: AccessClaims,
    pub request_id: String,
}

/// Rejection for auth middleware (stable API error body).
pub struct AuthRejection {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    request_id: String,
}

impl AuthRejection {
    fn new(
        status: StatusCode,
        code: &'static str,
        message: &'static str,
        request_id: String,
    ) -> Self {
        Self {
            status,
            code,
            message,
            request_id,
        }
    }
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiError {
                code: self.code.into(),
                message: self.message.into(),
                request_id: self.request_id,
                details: None,
            }),
        )
            .into_response()
    }
}

fn request_id_from(parts: &Parts) -> String {
    // Prefer middleware-validated/generated request id; never invent from raw headers here.
    parts
        .extensions
        .get::<crate::middleware::RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn bearer_token(parts: &Parts) -> Result<&str, ()> {
    let header = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(())?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or(())?;
    if token.is_empty() || token.len() > 4096 {
        return Err(());
    }
    Ok(token)
}

impl FromRequestParts<Arc<crate::http::AppState>> for AuthenticatedOrg {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<crate::http::AppState>,
    ) -> Result<Self, Self::Rejection> {
        let request_id = request_id_from(parts);
        let provider = state.auth_provider().ok_or_else(|| {
            AuthRejection::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "auth_unavailable",
                "Authentication is not configured",
                request_id.clone(),
            )
        })?;

        let token = bearer_token(parts).map_err(|_| {
            AuthRejection::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Missing or invalid bearer token",
                request_id.clone(),
            )
        })?;

        // Do not log the token.
        let claims = provider.keys().verify_access_token(token).map_err(|_| {
            AuthRejection::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Access token is invalid or expired",
                request_id.clone(),
            )
        })?;

        let user_id = Uuid::parse_str(&claims.sub).map_err(|_| {
            AuthRejection::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Access token subject is invalid",
                request_id.clone(),
            )
        })?;
        let org_id = Uuid::parse_str(&claims.org_id).map_err(|_| {
            AuthRejection::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Access token org claim is invalid",
                request_id.clone(),
            )
        })?;

        // Authorization is current PG state — JWT org/user are hints only.
        let context = match resolve_org_context_in_txn(provider.pool(), org_id, user_id).await {
            Ok(context) => context,
            Err(error) => {
                let rejection = map_resolve_error(error, &request_id);
                let reason = match rejection.code {
                    "permission_denied" => crate::services::audit::AuditReason::PermissionDenied,
                    "membership_missing" => crate::services::audit::AuditReason::MembershipMissing,
                    "user_disabled" => crate::services::audit::AuditReason::UserDisabled,
                    "collection_denied" => crate::services::audit::AuditReason::CollectionDenied,
                    _ => crate::services::audit::AuditReason::InvalidCredentials,
                };
                let _ = crate::services::audit::write_deny_durable(
                    provider.pool(),
                    org_id,
                    Some(user_id),
                    crate::services::audit::AuditAction::AuthDeny,
                    crate::services::audit::AuditResource::Session,
                    None,
                    &request_id,
                    crate::services::audit::reason_metadata(reason),
                )
                .await;
                return Err(rejection);
            }
        };

        crate::telemetry::enrich_actor(context.org_id(), context.user_id());
        let auth = Self {
            context,
            claims,
            request_id,
        };
        // Stash for Path/Query/Json extractors that map rejections to ApiError.
        parts.extensions.insert(auth.clone());
        Ok(auth)
    }
}

fn map_resolve_error(error: ResolveError, request_id: &str) -> AuthRejection {
    let (status, code, message) = match error {
        ResolveError::UserDisabled => (
            StatusCode::FORBIDDEN,
            "user_disabled",
            "User account is disabled",
        ),
        ResolveError::MembershipMissing => (
            StatusCode::FORBIDDEN,
            "membership_missing",
            "Org membership is missing",
        ),
        ResolveError::PermissionDenied => (
            StatusCode::FORBIDDEN,
            "permission_denied",
            "Permission denied",
        ),
        ResolveError::CollectionDenied => (
            StatusCode::FORBIDDEN,
            "collection_denied",
            "Collection access denied",
        ),
        ResolveError::InvalidContext | ResolveError::Database => (
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Unable to resolve organization context",
        ),
    };
    tracing::info!(
        target: "auth",
        request_id = %request_id,
        code,
        outcome = "deny",
        "auth resolve denied"
    );
    crate::telemetry::record_auth_decision("deny", code);
    AuthRejection::new(status, code, message, request_id.to_string())
}

/// Maps session errors to HTTP responses without embedding secrets.
pub fn session_error_response(error: SessionError, request_id: &str) -> Response {
    let (status, code, message) = match error {
        SessionError::InvalidCredentials => (
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "Email or password is incorrect",
        ),
        SessionError::UserDisabled => (
            StatusCode::FORBIDDEN,
            "user_disabled",
            "User account is disabled",
        ),
        SessionError::MembershipMissing => (
            StatusCode::FORBIDDEN,
            "membership_missing",
            "Org membership is missing",
        ),
        SessionError::InvalidRefresh => (
            StatusCode::UNAUTHORIZED,
            "invalid_refresh",
            "Refresh token is invalid or expired",
        ),
        SessionError::RefreshReuse => (
            StatusCode::UNAUTHORIZED,
            "refresh_reuse",
            "Refresh token reuse detected; session family revoked",
        ),
        SessionError::NotConfigured => (
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "Authentication is not configured",
        ),
        SessionError::Database | SessionError::Token | SessionError::Password => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Authentication failed",
        ),
    };
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: message.into(),
            request_id: request_id.to_string(),
            details: None,
        }),
    )
        .into_response()
}

/// Verify-only helper for tests that do not go through axum extractors.
pub fn verify_bearer_claims(
    keys: &JwtKeys,
    authorization: &str,
) -> Result<AccessClaims, SessionError> {
    let token = authorization
        .strip_prefix("Bearer ")
        .or_else(|| authorization.strip_prefix("bearer "))
        .ok_or(SessionError::InvalidCredentials)?;
    keys.verify_access_token(token)
        .map_err(|_| SessionError::Token)
}
