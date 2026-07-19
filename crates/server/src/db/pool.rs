//! Connection pool and transaction-local RLS helpers (ADR 0007).

use std::future::Future;
use std::pin::Pin;

use deadpool_postgres::{Config, Pool, PoolConfig, Runtime};
use tokio_postgres::{NoTls, Transaction};

use crate::auth::context::OrgContext;
use crate::database::{database_requires_tls, make_rustls_connect};
use crate::db::error::DbError;

/// Future returned by [`with_org_txn`] closures (boxed to satisfy HRTB + borrow).
pub type OrgTxnFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, DbError>> + Send + 'a>>;

/// Typed future returned by [`with_org_txn_typed`] closures.
pub type OrgTxnTypedFuture<'a, T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// Creates a deadpool-postgres pool with the default max size.
pub fn create_pool(database_url: &str) -> Result<Pool, DbError> {
    create_pool_with_max_size(database_url, PoolConfig::default().max_size)
}

/// Creates a pool with an explicit max size (use `1` to force connection reuse in tests).
pub fn create_pool_with_max_size(database_url: &str, max_size: usize) -> Result<Pool, DbError> {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());
    cfg.pool = Some(PoolConfig::new(max_size));
    if database_requires_tls(database_url).map_err(DbError::Config)? {
        let connector = make_rustls_connect().map_err(DbError::Config)?;
        cfg.create_pool(Some(Runtime::Tokio1), connector)
            .map_err(|error| DbError::Config(error.to_string()))
    } else {
        cfg.create_pool(Some(Runtime::Tokio1), NoTls)
            .map_err(|error| DbError::Config(error.to_string()))
    }
}

/// Runs `f` inside a transaction with transaction-local RLS claims set.
///
/// Sets only `SET LOCAL` / `set_config(..., is_local=true)` for `app.org_id` and
/// `app.user_id` so pooled connections never retain tenant state after commit or
/// rollback. The closure must not perform network, converter, or LLM I/O.
///
/// Call sites should return `Box::pin(async move { ... })`.
pub async fn with_org_txn<T, F>(pool: &Pool, ctx: &OrgContext, f: F) -> Result<T, DbError>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> OrgTxnFuture<'c, T>,
{
    let mut client = pool.get().await?;
    let txn = client.transaction().await?;
    apply_org_context(&txn, ctx).await?;
    match f(&txn).await {
        Ok(value) => {
            txn.commit().await?;
            Ok(value)
        }
        Err(error) => {
            let _ = txn.rollback().await;
            Err(error)
        }
    }
}

/// Runs `f` inside an org transaction while preserving service-specific errors.
///
/// This has the same RLS/session semantics as [`with_org_txn`]; it exists for
/// service layers that need typed, non-database errors to trigger rollback.
pub async fn with_org_txn_typed<T, F, E>(pool: &Pool, ctx: &OrgContext, f: F) -> Result<T, E>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> OrgTxnTypedFuture<'c, T, E>,
    E: From<DbError>,
{
    let mut client = pool.get().await.map_err(DbError::from).map_err(E::from)?;
    let txn = client
        .transaction()
        .await
        .map_err(DbError::from)
        .map_err(E::from)?;
    apply_org_context(&txn, ctx).await.map_err(E::from)?;
    match f(&txn).await {
        Ok(value) => {
            txn.commit().await.map_err(DbError::from).map_err(E::from)?;
            Ok(value)
        }
        Err(error) => {
            let _ = txn.rollback().await;
            Err(error)
        }
    }
}

/// Applies tenant claims as transaction-local GUCs (never session-level).
pub async fn apply_org_context(txn: &Transaction<'_>, ctx: &OrgContext) -> Result<(), DbError> {
    let org = ctx.org_id().to_string();
    let user = ctx.user_id().to_string();
    txn.execute("SELECT set_config('app.org_id', $1, true)", &[&org])
        .await?;
    txn.execute("SELECT set_config('app.user_id', $1, true)", &[&user])
        .await?;
    Ok(())
}
