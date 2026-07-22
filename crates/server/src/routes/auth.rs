//! Authentication HTTP routes (bearer JWT access + opaque rotating refresh).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::{ApiError, ApiRejection, AppJson};
use crate::auth::middleware::{session_error_response, AuthenticatedOrg};
use crate::auth::provider::{AuthProvider, AuthRequestMeta};
use crate::http::AppState;
use crate::middleware::ResolvedRequestId;

const MAX_EMAIL_LEN: usize = 320;
const MAX_PASSWORD_LEN: usize = 1024;
const MAX_REFRESH_LEN: usize = 512;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest")
            .field("email", &self.email)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRequest {
    pub refresh_token: String,
}

impl std::fmt::Debug for RefreshRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshRequest")
            .field("refresh_token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogoutRequest {
    pub refresh_token: String,
}

impl std::fmt::Debug for LogoutRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogoutRequest")
            .field("refresh_token", &"[REDACTED]")
            .finish()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: &'static str,
    expires_in: u64,
    org_id: String,
    user_id: String,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("org_id", &self.org_id)
            .field("user_id", &self.user_id)
            .finish()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MeResponse {
    user_id: String,
    org_id: String,
    email: String,
    display_name: String,
    permissions: Vec<String>,
    allowed_collection_ids: Vec<String>,
    session_id: String,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me))
}

fn validation_error(request_id: &str, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            code: "validation_failed".into(),
            message: message.into(),
            request_id: request_id.to_string(),
            details: None,
        }),
    )
        .into_response()
}

async fn login(
    State(state): State<Arc<AppState>>,
    ResolvedRequestId(request_id): ResolvedRequestId,
    AppJson(body): AppJson<LoginRequest>,
) -> Result<Response, ApiRejection> {
    Ok(login_inner(state, request_id, body).await)
}

async fn login_inner(state: Arc<AppState>, request_id: String, body: LoginRequest) -> Response {
    if body.email.len() > MAX_EMAIL_LEN || body.password.len() > MAX_PASSWORD_LEN {
        return validation_error(&request_id, "Email or password exceeds allowed length");
    }
    if body.email.trim().is_empty() || body.password.is_empty() {
        return validation_error(&request_id, "Email and password are required");
    }
    let Some(provider) = state.auth_provider() else {
        return session_error_response(
            crate::auth::session::SessionError::NotConfigured,
            &request_id,
        );
    };
    let meta = AuthRequestMeta {
        request_id: request_id.clone(),
    };
    match provider
        .login_password(&body.email, &body.password, &meta)
        .await
    {
        Ok(session) => {
            let tokens = session.tokens;
            Json(TokenResponse {
                access_token: tokens.access_token.expose().to_string(),
                refresh_token: tokens.refresh_token.expose().to_string(),
                token_type: "Bearer",
                expires_in: tokens.expires_in,
                org_id: tokens.org_id.to_string(),
                user_id: tokens.user_id.to_string(),
            })
            .into_response()
        }
        Err(error) => session_error_response(error, &request_id),
    }
}

async fn refresh(
    State(state): State<Arc<AppState>>,
    ResolvedRequestId(request_id): ResolvedRequestId,
    AppJson(body): AppJson<RefreshRequest>,
) -> Result<Response, ApiRejection> {
    Ok(refresh_inner(state, request_id, body).await)
}

async fn refresh_inner(state: Arc<AppState>, request_id: String, body: RefreshRequest) -> Response {
    if body.refresh_token.is_empty() || body.refresh_token.len() > MAX_REFRESH_LEN {
        return validation_error(&request_id, "refreshToken is required");
    }
    let Some(provider) = state.auth_provider() else {
        return session_error_response(
            crate::auth::session::SessionError::NotConfigured,
            &request_id,
        );
    };
    let meta = AuthRequestMeta {
        request_id: request_id.clone(),
    };
    match provider.refresh(&body.refresh_token, &meta).await {
        Ok(session) => {
            let tokens = session.tokens;
            Json(TokenResponse {
                access_token: tokens.access_token.expose().to_string(),
                refresh_token: tokens.refresh_token.expose().to_string(),
                token_type: "Bearer",
                expires_in: tokens.expires_in,
                org_id: tokens.org_id.to_string(),
                user_id: tokens.user_id.to_string(),
            })
            .into_response()
        }
        Err(error) => session_error_response(error, &request_id),
    }
}

async fn logout(
    State(state): State<Arc<AppState>>,
    ResolvedRequestId(request_id): ResolvedRequestId,
    AppJson(body): AppJson<LogoutRequest>,
) -> Result<Response, ApiRejection> {
    Ok(logout_inner(state, request_id, body).await)
}

async fn logout_inner(state: Arc<AppState>, request_id: String, body: LogoutRequest) -> Response {
    if body.refresh_token.is_empty() || body.refresh_token.len() > MAX_REFRESH_LEN {
        return validation_error(&request_id, "refreshToken is required");
    }
    let Some(provider) = state.auth_provider() else {
        return session_error_response(
            crate::auth::session::SessionError::NotConfigured,
            &request_id,
        );
    };
    let meta = AuthRequestMeta {
        request_id: request_id.clone(),
    };
    match provider.logout(&body.refresh_token, &meta).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => session_error_response(error, &request_id),
    }
}

async fn me(State(state): State<Arc<AppState>>, auth: AuthenticatedOrg) -> Response {
    let Some(provider) = state.auth_provider() else {
        return session_error_response(
            crate::auth::session::SessionError::NotConfigured,
            &auth.request_id,
        );
    };
    match crate::auth::session::load_user_profile(provider.pool(), auth.context.user_id()).await {
        Ok((email, display_name)) => Json(MeResponse {
            user_id: auth.context.user_id().to_string(),
            org_id: auth.context.org_id().to_string(),
            email,
            display_name,
            permissions: auth.context.permissions().iter().cloned().collect(),
            allowed_collection_ids: auth
                .context
                .allowed_collection_ids()
                .iter()
                .map(ToString::to_string)
                .collect(),
            session_id: auth.claims.sid,
        })
        .into_response(),
        Err(error) => session_error_response(error, &auth.request_id),
    }
}
