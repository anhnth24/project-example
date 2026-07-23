//! In-process token-bucket rate limiter (P1B-R06). Not distributed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HARD_CAP_KEYS: usize = 10_000;

#[derive(Debug, Clone, Copy)]
pub struct RateLimitConfig {
    pub auth_per_minute: u32,
    pub user_per_minute: u32,
    pub ip_per_minute: u32,
    pub expensive_route_per_minute: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            auth_per_minute: 30,
            user_per_minute: 120,
            ip_per_minute: 240,
            expensive_route_per_minute: 60,
        }
    }
}

impl RateLimitConfig {
    /// Parse env overrides; fail fast on zero/invalid values.
    pub fn from_env() -> Result<Self, String> {
        let mut config = Self::default();
        if let Some(value) = env_u32("MARKHAND_RATE_AUTH_PER_MINUTE")? {
            config.auth_per_minute = value;
        }
        if let Some(value) = env_u32("MARKHAND_RATE_USER_PER_MINUTE")? {
            config.user_per_minute = value;
        }
        if let Some(value) = env_u32("MARKHAND_RATE_IP_PER_MINUTE")? {
            config.ip_per_minute = value;
        }
        if let Some(value) = env_u32("MARKHAND_RATE_ROUTE_PER_MINUTE")? {
            config.expensive_route_per_minute = value;
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(self) -> Result<Self, String> {
        for (name, value) in [
            ("auth_per_minute", self.auth_per_minute),
            ("user_per_minute", self.user_per_minute),
            ("ip_per_minute", self.ip_per_minute),
            (
                "expensive_route_per_minute",
                self.expensive_route_per_minute,
            ),
        ] {
            if value == 0 {
                return Err(format!("rate limit {name} must be >= 1"));
            }
        }
        Ok(self)
    }
}

fn env_u32(name: &str) -> Result<Option<u32>, String> {
    match std::env::var(name) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(format!("{name} is empty"));
            }
            trimmed
                .parse::<u32>()
                .map(Some)
                .map_err(|_| format!("{name} must be a positive integer"))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("{name}: {error}")),
    }
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
    capacity: f64,
    refill_per_sec: f64,
}

impl Bucket {
    fn new(capacity: u32) -> Self {
        let capacity = f64::from(capacity.max(1));
        Self {
            tokens: capacity,
            last: Instant::now(),
            capacity,
            refill_per_sec: capacity / 60.0,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last = now;
    }

    fn take_at(&mut self, now: Instant) -> Result<(), Duration> {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            let need = 1.0 - self.tokens;
            let secs = need / self.refill_per_sec;
            Err(Duration::from_secs_f64(secs.max(0.05)))
        }
    }

    /// Idle + full buckets are safe to drop without wiping active limits.
    fn is_evictable(&self, now: Instant) -> bool {
        now.duration_since(self.last) > Duration::from_secs(120)
            && self.tokens >= self.capacity - f64::EPSILON
    }
}

/// Optional deterministic clock for tests (shared body/header Retry-After math).
pub type ClockFn = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Bucket>>>,
    config: RateLimitConfig,
    clock: ClockFn,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimitConfig::default())
    }
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            config,
            clock: Arc::new(Instant::now),
        }
    }

    /// Test helper: inject a deterministic clock.
    pub fn with_clock(mut self, clock: ClockFn) -> Self {
        self.clock = clock;
        self
    }

    pub fn config(&self) -> RateLimitConfig {
        self.config
    }

    pub fn check_auth_ip(&self, ip: &str) -> Result<(), Duration> {
        self.check(&format!("auth:{ip}"), self.config.auth_per_minute)
    }

    pub fn check_ip(&self, ip: &str) -> Result<(), Duration> {
        self.check(&format!("ip:{ip}"), self.config.ip_per_minute)
    }

    pub fn check_user(&self, org_id: &str, user_id: &str) -> Result<(), Duration> {
        self.check(
            &format!("user:{org_id}:{user_id}"),
            self.config.user_per_minute,
        )
    }

    /// Per-route bucket keyed by peer IP (or user+route when `subject` is set).
    pub fn check_route(&self, route: &str, subject: &str) -> Result<(), Duration> {
        self.check(
            &format!("route:{route}:{subject}"),
            self.config.expensive_route_per_minute,
        )
    }

    pub fn len_for_test(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    fn prune_locked(guard: &mut HashMap<String, Bucket>, now: Instant) {
        guard.retain(|_, bucket| !bucket.is_evictable(now));
        if guard.len() > HARD_CAP_KEYS {
            let mut idle: Vec<(String, Instant)> =
                guard.iter().map(|(k, b)| (k.clone(), b.last)).collect();
            idle.sort_by_key(|(_, last)| *last);
            let remove_n = guard.len().saturating_sub(8_000);
            let protect: std::collections::HashSet<String> = guard
                .iter()
                .filter(|(_, bucket)| bucket.tokens < bucket.capacity - f64::EPSILON)
                .map(|(key, _)| key.clone())
                .collect();
            for (key, _) in idle.into_iter().take(remove_n) {
                if protect.contains(&key) {
                    continue;
                }
                guard.remove(&key);
            }
        }
    }

    fn check(&self, key: &str, capacity: u32) -> Result<(), Duration> {
        let mut guard = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let now = (self.clock)();
        if guard.len() >= HARD_CAP_KEYS {
            Self::prune_locked(&mut guard, now);
        }
        // At hard cap, refuse *unseen* keys without inserting (preserve active buckets).
        if guard.len() >= HARD_CAP_KEYS && !guard.contains_key(key) {
            return Err(Duration::from_secs(1));
        }
        let bucket = guard
            .entry(key.to_string())
            .or_insert_with(|| Bucket::new(capacity));
        bucket.take_at(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_enforces_capacity() {
        let limiter = RateLimiter::new(RateLimitConfig {
            auth_per_minute: 2,
            user_per_minute: 2,
            ip_per_minute: 2,
            expensive_route_per_minute: 2,
        });
        assert!(limiter.check_auth_ip("1.2.3.4").is_ok());
        assert!(limiter.check_auth_ip("1.2.3.4").is_ok());
        assert!(limiter.check_auth_ip("1.2.3.4").is_err());
    }

    #[test]
    fn config_rejects_zero_capacity() {
        assert!(RateLimitConfig {
            auth_per_minute: 0,
            ..RateLimitConfig::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn eviction_does_not_clear_active_bucket() {
        let limiter = RateLimiter::new(RateLimitConfig {
            auth_per_minute: 1,
            user_per_minute: 1,
            ip_per_minute: 1,
            expensive_route_per_minute: 1,
        });
        assert!(limiter.check_route("search", "10.0.0.1").is_ok());
        assert!(limiter.check_route("search", "10.0.0.1").is_err());
        {
            let mut guard = limiter
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for i in 0..10_050 {
                let mut bucket = Bucket::new(10);
                bucket.last = Instant::now() - Duration::from_secs(600);
                bucket.tokens = bucket.capacity;
                guard.insert(format!("idle:{i}"), bucket);
            }
        }
        assert!(limiter.check_route("search", "10.0.0.1").is_err());
        assert!(limiter.len_for_test() <= HARD_CAP_KEYS);
    }

    #[test]
    fn hard_cap_rejects_unseen_without_insert() {
        let limiter = RateLimiter::new(RateLimitConfig::default());
        {
            let mut guard = limiter
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for i in 0..HARD_CAP_KEYS {
                let mut bucket = Bucket::new(10);
                // Non-evictable: recently used, not full.
                bucket.last = Instant::now();
                bucket.tokens = 1.0;
                guard.insert(format!("busy:{i}"), bucket);
            }
        }
        let before = limiter.len_for_test();
        assert!(limiter.check_ip("203.0.113.99").is_err());
        assert_eq!(limiter.len_for_test(), before);
    }

    #[test]
    fn twenty_k_key_pressure_stays_bounded() {
        let limiter = RateLimiter::new(RateLimitConfig {
            auth_per_minute: 1_000,
            user_per_minute: 1_000,
            ip_per_minute: 1_000,
            expensive_route_per_minute: 1_000,
        });
        for i in 0..20_000 {
            let _ = limiter.check_ip(&format!("198.51.100.{}", i % 250));
            let _ = limiter.check_route("search", &format!("user-{i}"));
        }
        assert!(limiter.len_for_test() <= HARD_CAP_KEYS);
    }
}
