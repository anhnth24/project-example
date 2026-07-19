//! Auth provider interface so OIDC can later mint the same session shape.

use std::future::Future;
use std::pin::Pin;

use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::auth::session::{
    self, logout_session, refresh_session, revoke_all_user_families, SessionError, TokenPair,
};
use crate::config::AuthConfig;

/// Request metadata attached to auth operations (never includes secrets).
#[derive(Debug, Clone)]
pub struct AuthRequestMeta {
    pub request_id: String,
}

/// Shared session shape minted by every auth provider (password today, OIDC later).
#[derive(Debug, Clone)]
pub struct InternalSession {
    pub tokens: TokenPair,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Provider contract: password or OIDC may authenticate, but must mint sessions
/// through the same rotation / OrgContext / audit path — never bypass it.
pub trait AuthProvider: Send + Sync {
    /// Exchange first-party credentials for an internal session.
    fn login_password<'a>(
        &'a self,
        email: &'a str,
        password: &'a str,
        meta: &'a AuthRequestMeta,
    ) -> BoxFuture<'a, Result<InternalSession, SessionError>>;

    /// Rotate a refresh token (reuse revokes the family).
    fn refresh<'a>(
        &'a self,
        refresh_token: &'a str,
        meta: &'a AuthRequestMeta,
    ) -> BoxFuture<'a, Result<InternalSession, SessionError>>;

    /// Revoke the refresh token family.
    fn logout<'a>(
        &'a self,
        refresh_token: &'a str,
        meta: &'a AuthRequestMeta,
    ) -> BoxFuture<'a, Result<(), SessionError>>;

    /// Revoke every family for a user (disable / password-reset).
    fn revoke_user_sessions<'a>(
        &'a self,
        org_id: Uuid,
        user_id: Uuid,
        meta: &'a AuthRequestMeta,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<(), SessionError>>;
}

/// Phase 1B password auth provider.
pub struct PasswordAuthProvider {
    pool: Pool,
    auth: AuthConfig,
    keys: JwtKeys,
}

impl PasswordAuthProvider {
    pub fn new(pool: Pool, auth: AuthConfig, keys: JwtKeys) -> Self {
        Self { pool, auth, keys }
    }

    pub fn keys(&self) -> &JwtKeys {
        &self.keys
    }

    pub fn auth_config(&self) -> &AuthConfig {
        &self.auth
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }
}

impl AuthProvider for PasswordAuthProvider {
    fn login_password<'a>(
        &'a self,
        email: &'a str,
        password: &'a str,
        meta: &'a AuthRequestMeta,
    ) -> BoxFuture<'a, Result<InternalSession, SessionError>> {
        Box::pin(async move {
            let tokens = session::login_with_password(
                &self.pool,
                &self.auth,
                &self.keys,
                email,
                password,
                &meta.request_id,
            )
            .await?;
            Ok(InternalSession { tokens })
        })
    }

    fn refresh<'a>(
        &'a self,
        refresh_token: &'a str,
        meta: &'a AuthRequestMeta,
    ) -> BoxFuture<'a, Result<InternalSession, SessionError>> {
        Box::pin(async move {
            let tokens = refresh_session(
                &self.pool,
                &self.auth,
                &self.keys,
                refresh_token,
                &meta.request_id,
            )
            .await?;
            Ok(InternalSession { tokens })
        })
    }

    fn logout<'a>(
        &'a self,
        refresh_token: &'a str,
        meta: &'a AuthRequestMeta,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move { logout_session(&self.pool, refresh_token, &meta.request_id).await })
    }

    fn revoke_user_sessions<'a>(
        &'a self,
        org_id: Uuid,
        user_id: Uuid,
        meta: &'a AuthRequestMeta,
        reason: &'a str,
    ) -> BoxFuture<'a, Result<(), SessionError>> {
        Box::pin(async move {
            revoke_all_user_families(&self.pool, org_id, user_id, &meta.request_id, reason).await
        })
    }
}
