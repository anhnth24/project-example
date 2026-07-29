//! Bounded in-process cache for [`OrgContext`] resolution (1C-05).
//!
//! **Scope.** This cache wraps ONLY the top-level entry point used by the
//! `AuthenticatedOrg` axum extractor
//! (`auth::permissions::resolve_org_context_in_txn`, called once per
//! incoming HTTP request). It deliberately does NOT wrap:
//! - `auth::permissions::resolve_org_context_on_txn`, used inside an
//!   already-open, already-locked transaction (ask-stream append/pull) where
//!   the caller needs a snapshot that cannot be torn by a concurrent ACL
//!   writer — that path must stay always-fresh.
//! - `services::upload::saga::reload_principal_locked`, the upload saga's
//!   own re-authorization barrier taken right before commit — it already
//!   re-reads PostgreSQL directly under the principal advisory lock and must
//!   keep doing so; nothing here changes that.
//!
//! Both of those already run their own fresh queries every time, so neither
//! is a regression risk from adding this cache; see the session report for
//! the full "which query runs where" audit that justified this scope.
//!
//! **Invalidation design.** A cache hit is NEVER trusted blindly. Every hit
//! (within TTL) re-checks two cheap, non-RLS-protected point lookups —
//! `users.disabled_at` and `orgs.acl_version` — against PostgreSQL before
//! returning cached data. Only when `disabled_at IS NULL` and the org's
//! current `acl_version` still matches the version recorded at cache-fill
//! time is the cached `OrgContext` returned; anything else (mismatch, row
//! gone, the freshness query itself failing) falls through to a full,
//! authoritative `resolve_org_context_in_txn` call, which performs its own
//! typed fail-closed checks. A cache hit therefore never saves the
//! membership/permission/collection queries UNLESS a strictly fresher
//! signal already confirmed nothing relevant changed — this is what makes
//! revoke/suspend/remove effective on the very next request, exactly like
//! before this cache existed (see `db::orgs::bump_acl_version` for who bumps
//! the version and `plans/markhand-web/backlog/phase-1c/issues/README.md`
//! 1C-05 for the acceptance this satisfies).
//!
//! **Cross-instance.** Because every hit re-validates against PostgreSQL
//! (never a blind TTL-only trust), a version bump committed by one server
//! process is visible to every other process on ITS very next request too —
//! there is no separate cross-instance invalidation channel to build or
//! reason about. This is why 1C-05's "cross-instance invalidation
//! semantics" concern does not need to be deferred: the design never
//! actually trusts a cache entry without asking PostgreSQL first.
//!
//! **TTL.** The TTL bounds two residual risks, not the common case: (a) any
//! future mutation path someone forgets to wire to `bump_acl_version`, and
//! (b) the KNOWN GAP documented on migration `0031` — collection
//! create/soft-delete via `db::collections` does not bump the version yet.
//! Once TTL elapses, the entry is dropped from consideration and a full
//! fresh resolve runs unconditionally, regardless of whether the version
//! still matches.
//!
//! **Fail-closed.** Any ambiguity (DB error on the freshness check, row
//! missing) is treated as "do not trust the cache", never as "deny the
//! request" and never as "trust the cache anyway" — the authoritative
//! resolve below is what actually produces the request's allow/deny
//! decision.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use uuid::Uuid;

use crate::auth::context::OrgContext;
use crate::auth::permissions::{resolve_org_context_in_txn, ResolveError};
use crate::db::error::DbError;

/// Max distinct `(org_id, user_id)` principals held at once. Eviction beyond
/// this is never a correctness concern, only a cache miss.
pub const DEFAULT_CAPACITY: usize = 4096;

/// How long a cache hit is trusted before being dropped unconditionally
/// (see module doc "TTL"). Short on purpose. Not (yet) operator-configurable
/// — see session report for why this was kept simple in this round.
pub const DEFAULT_TTL: Duration = Duration::from_secs(3);

#[derive(Clone)]
struct Entry {
    context: OrgContext,
    acl_version: i64,
    resolved_at: Instant,
}

/// Bounded, TTL + version-checked cache of resolved [`OrgContext`] values.
///
/// Cheap to construct; owned by [`crate::auth::provider::PasswordAuthProvider`]
/// (one instance per process/`AppState`).
pub struct OrgContextCache {
    capacity: usize,
    ttl: Duration,
    entries: Mutex<HashMap<(Uuid, Uuid), Entry>>,
}

impl Default for OrgContextCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY, DEFAULT_TTL)
    }
}

impl OrgContextCache {
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            capacity: capacity.max(1),
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Resolves `OrgContext` for `(org_id, user_id)`, consulting the cache
    /// first but never trusting it without a same-request freshness check
    /// against PostgreSQL. See module doc.
    pub async fn resolve(
        &self,
        pool: &Pool,
        org_id: Uuid,
        user_id: Uuid,
    ) -> Result<OrgContext, ResolveError> {
        let key = (org_id, user_id);
        let cached = {
            let guard = self.entries.lock().expect("org context cache poisoned");
            guard.get(&key).cloned()
        };
        if let Some(entry) = cached {
            let age = entry.resolved_at.elapsed();
            if should_check_freshness(age, self.ttl) {
                if let Ok(Some((disabled_at, current_version))) =
                    freshness_check(pool, org_id, user_id).await
                {
                    if trusts_cache(disabled_at, current_version, entry.acl_version) {
                        return Ok(entry.context);
                    }
                }
                // Disabled, version mismatch, missing row, or the freshness
                // query itself erroring all fall through to the
                // authoritative resolve below. Never treat any of these as
                // a deny by themselves — only the real resolve decides that.
            }
        }

        let fresh = resolve_org_context_in_txn(pool, org_id, user_id).await?;
        if let Ok(Some(version)) = current_acl_version(pool, org_id).await {
            self.insert(key, fresh.clone(), version);
        }
        // If the version itself could not be read, the resolution is simply
        // not cached (still returns the correct, freshly-resolved context):
        // an uncached entry can never serve stale data.
        Ok(fresh)
    }

    fn insert(&self, key: (Uuid, Uuid), context: OrgContext, acl_version: i64) {
        let mut guard = self.entries.lock().expect("org context cache poisoned");
        if guard.len() >= self.capacity && !guard.contains_key(&key) {
            // Bounded, not a strict LRU: any single eviction is safe (a
            // miss just re-resolves), so a cheap "evict one arbitrary
            // entry" policy is preferred over extra bookkeeping.
            if let Some(evict_key) = guard.keys().next().copied() {
                guard.remove(&evict_key);
            }
        }
        guard.insert(
            key,
            Entry {
                context,
                acl_version,
                resolved_at: Instant::now(),
            },
        );
    }

    /// Drops every cached principal. Exposed for ops/tests; production code
    /// does not need to call this (invalidation is version/TTL driven).
    pub fn clear(&self) {
        self.entries
            .lock()
            .expect("org context cache poisoned")
            .clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

/// Pure: whether a cache hit within `age` should even attempt the freshness
/// check, vs. being dropped outright once `ttl` has elapsed.
fn should_check_freshness(age: Duration, ttl: Duration) -> bool {
    age < ttl
}

/// Pure: whether the cached entry may be returned given the freshness
/// signal just read from PostgreSQL. Fail-closed: disabled or mismatched
/// version never trusts the cache.
fn trusts_cache(
    disabled_at: Option<DateTime<Utc>>,
    current_version: i64,
    cached_version: i64,
) -> bool {
    disabled_at.is_none() && current_version == cached_version
}

async fn freshness_check(
    pool: &Pool,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<Option<(Option<DateTime<Utc>>, i64)>, DbError> {
    let client = pool.get().await?;
    let row = client
        .query_opt(
            "SELECT u.disabled_at, o.acl_version
             FROM users u, orgs o
             WHERE u.id = $1 AND o.id = $2",
            &[&user_id, &org_id],
        )
        .await?;
    Ok(row.map(|row| (row.get(0), row.get(1))))
}

async fn current_acl_version(pool: &Pool, org_id: Uuid) -> Result<Option<i64>, DbError> {
    let client = pool.get().await?;
    let row = client
        .query_opt("SELECT acl_version FROM orgs WHERE id = $1", &[&org_id])
        .await?;
    Ok(row.map(|row| row.get(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusts_cache_only_when_enabled_and_version_matches() {
        assert!(trusts_cache(None, 5, 5));
        assert!(!trusts_cache(Some(Utc::now()), 5, 5), "disabled must deny");
        assert!(!trusts_cache(None, 6, 5), "version bump must invalidate");
        assert!(!trusts_cache(None, 4, 5), "any mismatch must invalidate");
    }

    #[test]
    fn should_check_freshness_respects_ttl() {
        let ttl = Duration::from_secs(3);
        assert!(should_check_freshness(Duration::from_millis(500), ttl));
        assert!(!should_check_freshness(Duration::from_secs(3), ttl));
        assert!(!should_check_freshness(Duration::from_secs(10), ttl));
    }

    #[test]
    fn bounded_capacity_evicts_rather_than_grows_unboundedly() {
        let cache = OrgContextCache::new(2, Duration::from_secs(60));
        let org = Uuid::new_v4();
        let ctx = |user: Uuid| OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
        cache.insert((org, Uuid::new_v4()), ctx(Uuid::new_v4()), 1);
        cache.insert((org, Uuid::new_v4()), ctx(Uuid::new_v4()), 1);
        assert_eq!(cache.len(), 2);
        cache.insert((org, Uuid::new_v4()), ctx(Uuid::new_v4()), 1);
        assert_eq!(cache.len(), 2, "capacity must stay bounded");
    }

    #[test]
    fn clear_drops_every_entry() {
        let cache = OrgContextCache::new(8, Duration::from_secs(60));
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let ctx = OrgContext::try_new(org, user, [] as [&str; 0], []).unwrap();
        cache.insert((org, user), ctx, 1);
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }
}
