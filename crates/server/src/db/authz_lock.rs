//! Dedicated advisory-lock connections for Q&A delivery/mutation barriers.
//!
//! Delivery holds **shared** session locks on a dedicated (non-pooled) connection.
//! Mutations take **exclusive** transaction locks on a separate mutation-capacity
//! slot before bumping epochs and committing — waiting for active deliveries.
//!
//! Advisory locks use `pg_try_advisory_*` + session/`SET LOCAL lock_timeout`
//! with a cancellable retry loop. On timeout the caller receives
//! [`DbError::LockTimeout`] and **fails closed** — the mutation/delivery slot is
//! released immediately so capacity is never starved waiting on a blocked peer.
//! Socket write deadlines for protected delivery live on the Q&A HTTP body adapter
//! ([`crate::services::qa::GuardedSseBody`]); they similarly fail (abandon guard)
//! rather than hold shared locks indefinitely.
//!
//! Drop of a [`DeliveryGuard`] abandons the dedicated connection (no blocking
//! unlock). PostgreSQL releases session locks when the backend disconnects.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_postgres::error::SqlState;
use tokio_postgres::{Client, Error as PgError, NoTls, Transaction};
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::database::{database_requires_tls, make_rustls_connect};
use crate::db::error::DbError;
use crate::db::pool::{apply_org_context, apply_org_context_on_client};

/// Two-int advisory lock key (PostgreSQL `pg_advisory_*($1,$2)`).
pub type AdvisoryKey = (i32, i32);

/// Namespace tag so QA locks never collide with other advisory users.
const QA_LOCK_TAG: u64 = 0x51A0_0A01;

/// Default concurrent delivery (shared) lock connections.
pub const DEFAULT_DELIVERY_LOCK_CAPACITY: usize = 64;
/// Default concurrent mutation (exclusive) lock connections — separate so
/// revocation/publish/delete are never starved by many deliveries.
pub const DEFAULT_MUTATION_LOCK_CAPACITY: usize = 16;
/// Max wait to obtain a lock-connection slot.
pub const DEFAULT_LOCK_ACQUIRE_DEADLINE: Duration = Duration::from_secs(5);
/// Connect timeout for a dedicated lock client.
pub const DEFAULT_LOCK_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// PG `lock_timeout` for blocking backends (defense in depth vs try-loop).
pub const DEFAULT_PG_LOCK_TIMEOUT: &str = "2s";
/// Poll interval for `pg_try_advisory_*` retry loop.
const ADVISORY_TRY_POLL: Duration = Duration::from_millis(25);

/// Map PostgreSQL `lock_timeout` / `55P03` to [`DbError::LockTimeout`].
fn map_pg_error(err: PgError) -> DbError {
    if err.code() == Some(&SqlState::LOCK_NOT_AVAILABLE) {
        return DbError::LockTimeout;
    }
    let msg = err.to_string().to_ascii_lowercase();
    if msg.contains("lock_timeout") || msg.contains("canceling statement") {
        return DbError::LockTimeout;
    }
    DbError::Query(err)
}

fn fold_uuid(id: Uuid) -> u64 {
    let b = id.as_u128();
    (b as u64) ^ ((b >> 64) as u64)
}

/// Membership/ACL lock for one org user.
pub fn user_lock_key(org_id: Uuid, user_id: Uuid) -> AdvisoryKey {
    let h = QA_LOCK_TAG
        ^ fold_uuid(org_id).rotate_left(7)
        ^ fold_uuid(user_id).rotate_left(17)
        ^ 0x5555_5555_5555_5555;
    ((h >> 32) as i32, h as i32)
}

/// Document mutation lock (tombstone/delete/publish).
pub fn document_lock_key(org_id: Uuid, document_id: Uuid) -> AdvisoryKey {
    let h = QA_LOCK_TAG
        ^ fold_uuid(org_id).rotate_left(3)
        ^ fold_uuid(document_id).rotate_left(23)
        ^ 0xAAAA_AAAA_AAAA_AAAA;
    ((h >> 32) as i32, h as i32)
}

/// Collection-scoped barrier (every delivery + visibility mutations).
pub fn collection_lock_key(org_id: Uuid, collection_id: Uuid) -> AdvisoryKey {
    let h = QA_LOCK_TAG
        ^ fold_uuid(org_id).rotate_left(5)
        ^ fold_uuid(collection_id).rotate_left(19)
        ^ 0xCCCC_DDDD_EEEE_FFFF;
    ((h >> 32) as i32, h as i32)
}

/// Stable group key — acquired by group membership/ACL mutations (H5).
pub fn group_lock_key(org_id: Uuid, group_id: Uuid) -> AdvisoryKey {
    let h = QA_LOCK_TAG
        ^ fold_uuid(org_id).rotate_left(9)
        ^ fold_uuid(group_id).rotate_left(21)
        ^ 0x1010_2020_3030_4040;
    ((h >> 32) as i32, h as i32)
}

/// Stable role key — acquired by role permission/ACL mutations (H5).
pub fn role_lock_key(org_id: Uuid, role_id: Uuid) -> AdvisoryKey {
    let h = QA_LOCK_TAG
        ^ fold_uuid(org_id).rotate_left(15)
        ^ fold_uuid(role_id).rotate_left(27)
        ^ 0xABCD_EF01_2345_6789;
    ((h >> 32) as i32, h as i32)
}

/// Writer-intent key: exclusive intent blocks new shared deliveries (no reader barging).
pub fn writer_intent_key(org_id: Uuid, document_id: Uuid) -> AdvisoryKey {
    let h = QA_LOCK_TAG
        ^ fold_uuid(org_id).rotate_left(11)
        ^ fold_uuid(document_id).rotate_left(29)
        ^ 0x1111_2222_3333_4444;
    ((h >> 32) as i32, h as i32)
}

/// User-scoped writer intent (membership/ACL mutations without document ids).
pub fn writer_intent_user_key(org_id: Uuid, user_id: Uuid) -> AdvisoryKey {
    let h = QA_LOCK_TAG
        ^ fold_uuid(org_id).rotate_left(13)
        ^ fold_uuid(user_id).rotate_left(31)
        ^ 0x5555_6666_7777_8888;
    ((h >> 32) as i32, h as i32)
}

/// Collection-scoped writer intent (visibility changes).
pub fn writer_intent_collection_key(org_id: Uuid, collection_id: Uuid) -> AdvisoryKey {
    let h = QA_LOCK_TAG
        ^ fold_uuid(org_id).rotate_left(17)
        ^ fold_uuid(collection_id).rotate_left(33)
        ^ 0x9999_AAAA_BBBB_CCCC;
    ((h >> 32) as i32, h as i32)
}

/// Dedup + globally sort delivery keys: user + documents + collections.
pub fn delivery_lock_keys(
    org_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
    collection_ids: &[Uuid],
) -> Vec<AdvisoryKey> {
    let mut keys = std::collections::BTreeSet::new();
    keys.insert(user_lock_key(org_id, user_id));
    for doc in document_ids {
        keys.insert(document_lock_key(org_id, *doc));
    }
    for cid in collection_ids {
        keys.insert(collection_lock_key(org_id, *cid));
    }
    keys.into_iter().collect()
}

/// Scope for exclusive mutation locks.
#[derive(Debug, Clone, Default)]
pub struct MutationLockScope {
    pub user_ids: Vec<Uuid>,
    pub document_ids: Vec<Uuid>,
    pub collection_ids: Vec<Uuid>,
    pub group_ids: Vec<Uuid>,
    pub role_ids: Vec<Uuid>,
}

/// Exclusive mutation keys (users/docs/collections/groups/roles), sorted.
pub fn mutation_lock_keys(org_id: Uuid, scope: &MutationLockScope) -> Vec<AdvisoryKey> {
    let mut keys = std::collections::BTreeSet::new();
    for uid in &scope.user_ids {
        keys.insert(user_lock_key(org_id, *uid));
    }
    for doc in &scope.document_ids {
        keys.insert(document_lock_key(org_id, *doc));
    }
    for cid in &scope.collection_ids {
        keys.insert(collection_lock_key(org_id, *cid));
    }
    for gid in &scope.group_ids {
        keys.insert(group_lock_key(org_id, *gid));
    }
    for rid in &scope.role_ids {
        keys.insert(role_lock_key(org_id, *rid));
    }
    keys.into_iter().collect()
}

/// Writer-intent keys for documents/users/collections (sorted).
pub fn writer_intent_keys(org_id: Uuid, scope: &MutationLockScope) -> Vec<AdvisoryKey> {
    let mut keys = std::collections::BTreeSet::new();
    for doc in &scope.document_ids {
        keys.insert(writer_intent_key(org_id, *doc));
    }
    for uid in &scope.user_ids {
        keys.insert(writer_intent_user_key(org_id, *uid));
    }
    for cid in &scope.collection_ids {
        keys.insert(writer_intent_collection_key(org_id, *cid));
    }
    keys.into_iter().collect()
}

/// Dedicated lock-connection factory (not the query pool).
#[derive(Clone)]
pub struct LockPool {
    database_url: String,
    delivery_slots: Arc<Semaphore>,
    mutation_slots: Arc<Semaphore>,
    acquire_deadline: Duration,
    connect_timeout: Duration,
}

impl std::fmt::Debug for LockPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LockPool")
            .field(
                "delivery_capacity",
                &self.delivery_slots.available_permits(),
            )
            .field(
                "mutation_capacity",
                &self.mutation_slots.available_permits(),
            )
            .field("acquire_deadline", &self.acquire_deadline)
            .finish_non_exhaustive()
    }
}

impl LockPool {
    pub fn new(database_url: &str) -> Result<Self, DbError> {
        Self::with_capacity(
            database_url,
            DEFAULT_DELIVERY_LOCK_CAPACITY,
            DEFAULT_MUTATION_LOCK_CAPACITY,
            DEFAULT_LOCK_ACQUIRE_DEADLINE,
            DEFAULT_LOCK_CONNECT_TIMEOUT,
        )
    }

    pub fn with_capacity(
        database_url: &str,
        delivery_capacity: usize,
        mutation_capacity: usize,
        acquire_deadline: Duration,
        connect_timeout: Duration,
    ) -> Result<Self, DbError> {
        if database_url.trim().is_empty() {
            return Err(DbError::Config("lock pool database URL empty".into()));
        }
        Ok(Self {
            database_url: database_url.to_string(),
            delivery_slots: Arc::new(Semaphore::new(delivery_capacity.max(1))),
            mutation_slots: Arc::new(Semaphore::new(mutation_capacity.max(1))),
            acquire_deadline,
            connect_timeout,
        })
    }

    async fn connect_client(&self) -> Result<Client, DbError> {
        let url = self.database_url.clone();
        let connect_timeout = self.connect_timeout;
        let requires_tls = database_requires_tls(&url).map_err(DbError::Config)?;
        let fut = async {
            if requires_tls {
                let connector = make_rustls_connect().map_err(DbError::Config)?;
                let (client, connection) = tokio_postgres::connect(&url, connector)
                    .await
                    .map_err(|e| DbError::Config(e.to_string()))?;
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                Ok(client)
            } else {
                let (client, connection) = tokio_postgres::connect(&url, NoTls)
                    .await
                    .map_err(|e| DbError::Config(e.to_string()))?;
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                Ok(client)
            }
        };
        tokio::time::timeout(connect_timeout, fut)
            .await
            .map_err(|_| DbError::Config("lock connection timeout".into()))?
    }

    async fn acquire_slot(&self, slots: &Arc<Semaphore>) -> Result<OwnedSemaphorePermit, DbError> {
        tokio::time::timeout(self.acquire_deadline, slots.clone().acquire_owned())
            .await
            .map_err(|_| DbError::LockTimeout)?
            .map_err(|_| DbError::Config("lock pool closed".into()))
    }

    pub fn acquire_deadline(&self) -> Duration {
        self.acquire_deadline
    }

    /// Acquire a dedicated delivery connection (shared-lock capacity).
    pub async fn acquire_delivery(&self) -> Result<LockConnection, DbError> {
        let permit = self.acquire_slot(&self.delivery_slots).await?;
        let client = match self.connect_client().await {
            Ok(c) => c,
            Err(e) => {
                drop(permit);
                return Err(e);
            }
        };
        Ok(LockConnection {
            client: Some(client),
            _permit: permit,
        })
    }

    /// Acquire a dedicated mutation connection (exclusive-lock capacity).
    pub async fn acquire_mutation(&self) -> Result<LockConnection, DbError> {
        let permit = self.acquire_slot(&self.mutation_slots).await?;
        let client = match self.connect_client().await {
            Ok(c) => c,
            Err(e) => {
                drop(permit);
                return Err(e);
            }
        };
        Ok(LockConnection {
            client: Some(client),
            _permit: permit,
        })
    }
}

/// Dedicated postgres client used only for advisory locks / barrier txns.
pub struct LockConnection {
    client: Option<Client>,
    _permit: OwnedSemaphorePermit,
}

impl LockConnection {
    pub fn client(&self) -> &Client {
        self.client.as_ref().expect("lock connection alive")
    }

    pub fn client_mut(&mut self) -> &mut Client {
        self.client.as_mut().expect("lock connection alive")
    }

    /// Close without returning to any pool (session locks released on disconnect).
    pub async fn abandon(mut self) {
        if let Some(client) = self.client.take() {
            // Dropping Client closes the connection; no unlock round-trip.
            drop(client);
        }
    }
}

impl Drop for LockConnection {
    fn drop(&mut self) {
        // Non-blocking abandon: drop Client handle; backend exit clears session locks.
        let _ = self.client.take();
    }
}

/// Shared session lock held through HTTP/SSE write until mark_sent/release.
pub struct DeliveryGuard {
    conn: Option<LockConnection>,
    keys: Vec<AdvisoryKey>,
    marked_sent: bool,
}

impl DeliveryGuard {
    /// Acquire **one** shared session guard for the entire HTTP response lifetime.
    ///
    /// H4: intent keys are try-once — if a writer holds exclusive intent, fail
    /// immediately with [`DbError::WriterIntent`] (no reader barging / waiting).
    /// Shared intent is unlocked immediately after the probe so writers can queue
    /// exclusive intent while this delivery still holds delivery keys.
    /// Delivery keys then use the try-loop deadline for the response lifetime.
    pub async fn acquire_shared(
        lock_pool: &LockPool,
        org_id: Uuid,
        user_id: Uuid,
        document_ids: &[Uuid],
        collection_ids: &[Uuid],
    ) -> Result<Self, DbError> {
        let keys = delivery_lock_keys(org_id, user_id, document_ids, collection_ids);
        let scope = MutationLockScope {
            user_ids: vec![user_id],
            document_ids: document_ids.to_vec(),
            collection_ids: collection_ids.to_vec(),
            ..MutationLockScope::default()
        };
        let intent_keys = writer_intent_keys(org_id, &scope);
        let deadline = Instant::now() + lock_pool.acquire_deadline();
        let conn = lock_pool.acquire_delivery().await?;
        apply_org_context_on_client(conn.client(), org_id, user_id).await?;
        conn.client()
            .execute(
                &format!("SET lock_timeout = '{DEFAULT_PG_LOCK_TIMEOUT}'"),
                &[],
            )
            .await
            .map_err(map_pg_error)?;
        // Intent probe only — do not hold shared intent for the response lifetime.
        for (k1, k2) in &intent_keys {
            let row = conn
                .client()
                .query_one("SELECT pg_try_advisory_lock_shared($1, $2)", &[k1, k2])
                .await;
            match row {
                Ok(r) if r.get::<_, bool>(0) => {
                    let _ = conn
                        .client()
                        .execute("SELECT pg_advisory_unlock_shared($1, $2)", &[k1, k2])
                        .await;
                }
                Ok(_) => {
                    conn.abandon().await;
                    return Err(DbError::WriterIntent);
                }
                Err(err) => {
                    conn.abandon().await;
                    return Err(map_pg_error(err));
                }
            }
        }
        for (k1, k2) in &keys {
            if let Err(err) = acquire_shared_key_try_loop(conn.client(), *k1, *k2, deadline).await {
                conn.abandon().await;
                return Err(err);
            }
        }
        Ok(Self {
            conn: Some(conn),
            keys,
            marked_sent: false,
        })
    }

    /// Borrow the lock-holding client (reads must complete before release/abandon).
    pub fn client(&self) -> Result<&Client, DbError> {
        self.conn
            .as_ref()
            .map(|c| c.client())
            .ok_or_else(|| DbError::Config("delivery guard released".into()))
    }

    pub(crate) fn mark_sent(&mut self) {
        self.marked_sent = true;
    }

    pub fn is_marked_sent(&self) -> bool {
        self.marked_sent
    }

    /// Explicit async unlock + close (preferred after mark_sent).
    pub async fn release(mut self) {
        if let Some(conn) = self.conn.take() {
            for (k1, k2) in &self.keys {
                let _ = conn
                    .client()
                    .execute("SELECT pg_advisory_unlock_shared($1, $2)", &[k1, k2])
                    .await;
            }
            conn.abandon().await;
        }
    }

    /// Abandon without unlock round-trip (connection close releases session locks).
    pub async fn abandon(mut self) {
        if let Some(conn) = self.conn.take() {
            conn.abandon().await;
        }
    }
}

async fn acquire_shared_key_try_loop(
    client: &Client,
    k1: i32,
    k2: i32,
    deadline: Instant,
) -> Result<(), DbError> {
    loop {
        let row = client
            .query_one("SELECT pg_try_advisory_lock_shared($1, $2)", &[&k1, &k2])
            .await?;
        let ok: bool = row.get(0);
        if ok {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(DbError::LockTimeout);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(ADVISORY_TRY_POLL.min(remaining)).await;
    }
}

async fn acquire_xact_key_try_loop(
    txn: &Transaction<'_>,
    k1: i32,
    k2: i32,
    deadline: Instant,
) -> Result<(), DbError> {
    loop {
        let row = txn
            .query_one("SELECT pg_try_advisory_xact_lock($1, $2)", &[&k1, &k2])
            .await?;
        let ok: bool = row.get(0);
        if ok {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(DbError::LockTimeout);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(ADVISORY_TRY_POLL.min(remaining)).await;
    }
}

/// Blocking xact lock; relies on `SET LOCAL lock_timeout` for bounded wait (H4).
async fn acquire_xact_key_blocking(txn: &Transaction<'_>, k1: i32, k2: i32) -> Result<(), DbError> {
    match txn
        .execute("SELECT pg_advisory_xact_lock($1, $2)", &[&k1, &k2])
        .await
    {
        Ok(_) => Ok(()),
        Err(err) => Err(map_pg_error(err)),
    }
}

impl Drop for DeliveryGuard {
    fn drop(&mut self) {
        // Non-blocking: abandon dedicated connection. No pool return, no sync wait.
        let _ = self.conn.take();
    }
}

type MutationFuture<'c, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, DbError>> + Send + 'c>>;

/// Central mutation API: blocking writer intent (PG `lock_timeout`), then drain
/// shared delivery locks, then body + commit.
pub async fn with_exclusive_mutation<T, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
    f: F,
) -> Result<T, DbError>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> MutationFuture<'c, T>,
{
    let scope = MutationLockScope {
        user_ids: vec![user_id],
        document_ids: document_ids.to_vec(),
        ..MutationLockScope::default()
    };
    with_exclusive_mutation_scope(lock_pool, ctx, org_id, &scope, f).await
}

/// Multi-user exclusive barrier (compat): users + documents only.
pub async fn with_exclusive_mutation_users<T, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_ids: &[Uuid],
    document_ids: &[Uuid],
    f: F,
) -> Result<T, DbError>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> MutationFuture<'c, T>,
{
    let scope = MutationLockScope {
        user_ids: user_ids.to_vec(),
        document_ids: document_ids.to_vec(),
        ..MutationLockScope::default()
    };
    with_exclusive_mutation_scope(lock_pool, ctx, org_id, &scope, f).await
}

/// Full-scope exclusive barrier.
pub async fn with_exclusive_mutation_scope<T, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    scope: &MutationLockScope,
    f: F,
) -> Result<T, DbError>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> MutationFuture<'c, T>,
{
    let mut scope = scope.clone();
    scope.user_ids.sort_unstable();
    scope.user_ids.dedup();
    scope.document_ids.sort_unstable();
    scope.document_ids.dedup();
    scope.collection_ids.sort_unstable();
    scope.collection_ids.dedup();
    scope.group_ids.sort_unstable();
    scope.group_ids.dedup();
    scope.role_ids.sort_unstable();
    scope.role_ids.dedup();
    if scope.user_ids.is_empty()
        && scope.document_ids.is_empty()
        && scope.collection_ids.is_empty()
        && scope.group_ids.is_empty()
        && scope.role_ids.is_empty()
    {
        return Err(DbError::Config(
            "mutation requires at least one lock key".into(),
        ));
    }
    let intent_keys = writer_intent_keys(org_id, &scope);
    let keys = mutation_lock_keys(org_id, &scope);
    let deadline = Instant::now() + lock_pool.acquire_deadline();
    let mut conn = lock_pool.acquire_mutation().await?;
    let result = {
        let txn = conn.client_mut().transaction().await?;
        apply_org_context(&txn, ctx).await?;
        txn.execute(
            &format!("SET LOCAL lock_timeout = '{DEFAULT_PG_LOCK_TIMEOUT}'"),
            &[],
        )
        .await?;
        // H4: queued blocking intent under PG lock_timeout — once held, new
        // readers fail-fast on try-shared intent.
        for (k1, k2) in &intent_keys {
            acquire_xact_key_blocking(&txn, *k1, *k2).await?;
        }
        // Drain active shared delivery holders.
        for (k1, k2) in &keys {
            acquire_xact_key_try_loop(&txn, *k1, *k2, deadline).await?;
        }
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
    };
    conn.abandon().await;
    result
}

type TypedMutationFuture<'c, T, E> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>> + Send + 'c>>;

/// Typed variant for service-layer errors (deletion/promotion/ACL).
pub async fn with_exclusive_mutation_typed<T, E, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_id: Uuid,
    document_ids: &[Uuid],
    f: F,
) -> Result<T, E>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> TypedMutationFuture<'c, T, E>,
    E: From<DbError>,
{
    let scope = MutationLockScope {
        user_ids: vec![user_id],
        document_ids: document_ids.to_vec(),
        ..MutationLockScope::default()
    };
    with_exclusive_mutation_scope_typed(lock_pool, ctx, org_id, &scope, f).await
}

/// Multi-user typed exclusive barrier (compat).
pub async fn with_exclusive_mutation_users_typed<T, E, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    user_ids: &[Uuid],
    document_ids: &[Uuid],
    f: F,
) -> Result<T, E>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> TypedMutationFuture<'c, T, E>,
    E: From<DbError>,
{
    let scope = MutationLockScope {
        user_ids: user_ids.to_vec(),
        document_ids: document_ids.to_vec(),
        ..MutationLockScope::default()
    };
    with_exclusive_mutation_scope_typed(lock_pool, ctx, org_id, &scope, f).await
}

/// Full-scope typed exclusive barrier.
pub async fn with_exclusive_mutation_scope_typed<T, E, F>(
    lock_pool: &LockPool,
    ctx: &OrgContext,
    org_id: Uuid,
    scope: &MutationLockScope,
    f: F,
) -> Result<T, E>
where
    F: for<'c> FnOnce(&'c Transaction<'c>) -> TypedMutationFuture<'c, T, E>,
    E: From<DbError>,
{
    let mut scope = scope.clone();
    scope.user_ids.sort_unstable();
    scope.user_ids.dedup();
    scope.document_ids.sort_unstable();
    scope.document_ids.dedup();
    scope.collection_ids.sort_unstable();
    scope.collection_ids.dedup();
    scope.group_ids.sort_unstable();
    scope.group_ids.dedup();
    scope.role_ids.sort_unstable();
    scope.role_ids.dedup();
    if scope.user_ids.is_empty()
        && scope.document_ids.is_empty()
        && scope.collection_ids.is_empty()
        && scope.group_ids.is_empty()
        && scope.role_ids.is_empty()
    {
        return Err(E::from(DbError::Config(
            "mutation requires at least one lock key".into(),
        )));
    }
    let intent_keys = writer_intent_keys(org_id, &scope);
    let keys = mutation_lock_keys(org_id, &scope);
    let deadline = Instant::now() + lock_pool.acquire_deadline();
    let mut conn = lock_pool.acquire_mutation().await.map_err(E::from)?;
    let result = {
        let txn = conn
            .client_mut()
            .transaction()
            .await
            .map_err(DbError::from)
            .map_err(E::from)?;
        apply_org_context(&txn, ctx).await.map_err(E::from)?;
        txn.execute(
            &format!("SET LOCAL lock_timeout = '{DEFAULT_PG_LOCK_TIMEOUT}'"),
            &[],
        )
        .await
        .map_err(DbError::from)
        .map_err(E::from)?;
        for (k1, k2) in &intent_keys {
            acquire_xact_key_blocking(&txn, *k1, *k2)
                .await
                .map_err(E::from)?;
        }
        for (k1, k2) in &keys {
            acquire_xact_key_try_loop(&txn, *k1, *k2, deadline)
                .await
                .map_err(E::from)?;
        }
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
    };
    conn.abandon().await;
    result
}
