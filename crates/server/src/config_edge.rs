//! CORS, trusted-proxy, and in-process rate-limit configuration (P1B-R06).

use std::str::FromStr;

use ipnet::IpNet;

use crate::config::Profile;

/// Configured CIDR allow-list for immediate peers that may set `X-Forwarded-For`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies {
    cidrs: Vec<IpNet>,
}

impl TrustedProxies {
    pub fn from_cidrs(cidrs: Vec<IpNet>) -> Self {
        Self { cidrs }
    }

    pub fn is_empty(&self) -> bool {
        self.cidrs.is_empty()
    }

    pub fn contains(&self, ip: std::net::IpAddr) -> bool {
        self.cidrs.iter().any(|cidr| cidr.contains(&ip))
    }
}

const DEFAULT_WINDOW_SECS: u64 = 60;
const MAX_WINDOW_SECS: u64 = 3_600;
const DEFAULT_MAX_KEYS: usize = 10_000;
const MAX_MAX_KEYS: usize = 100_000;
const DEFAULT_DEFAULT_IP_LIMIT: u32 = 120;
const DEFAULT_DEFAULT_USER_LIMIT: u32 = 600;
const DEFAULT_AUTH_IP_LIMIT: u32 = 30;
const DEFAULT_UPLOAD_IP_LIMIT: u32 = 30;
const DEFAULT_UPLOAD_USER_LIMIT: u32 = 60;
const DEFAULT_SEARCH_IP_LIMIT: u32 = 60;
const DEFAULT_SEARCH_USER_LIMIT: u32 = 180;
const DEFAULT_STREAM_IP_LIMIT: u32 = 30;
const DEFAULT_STREAM_USER_LIMIT: u32 = 60;
const DEFAULT_HEALTH_LIMIT: u32 = 1_000;
const MAX_LIMIT: u32 = 1_000_000;
const MAX_ORIGINS: usize = 64;
const MAX_CORS_LIST: usize = 32;

/// Exact-origin CORS policy (never silently widen in production).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub expose_headers: Vec<String>,
    pub allow_credentials: bool,
}

impl CorsConfig {
    pub fn production_defaults() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allowed_methods: vec![
                "GET".into(),
                "POST".into(),
                "PATCH".into(),
                "DELETE".into(),
                "HEAD".into(),
                "OPTIONS".into(),
            ],
            allowed_headers: vec![
                "Authorization".into(),
                "Content-Type".into(),
                "Idempotency-Key".into(),
                "X-Request-Id".into(),
                "Last-Event-ID".into(),
            ],
            expose_headers: vec![
                "X-Request-Id".into(),
                "Retry-After".into(),
                "X-RateLimit-Limit".into(),
                "X-RateLimit-Remaining".into(),
                "X-RateLimit-Reset".into(),
            ],
            allow_credentials: false,
        }
    }

    pub fn validate(&self, profile: Profile) -> Result<(), String> {
        if self.allowed_origins.len() > MAX_ORIGINS {
            return Err(format!(
                "MARKHAND_CORS_ORIGINS accepts at most {MAX_ORIGINS} exact origins"
            ));
        }
        for origin in &self.allowed_origins {
            if origin == "*" {
                return Err("CORS wildcard origin '*' is forbidden".into());
            }
            if origin.trim().is_empty() {
                return Err("MARKHAND_CORS_ORIGINS entries must be non-empty".into());
            }
            // Exact origins only: scheme://host[:port], no path/query/wildcard.
            let parsed = url::Url::parse(origin).map_err(|_| {
                "MARKHAND_CORS_ORIGINS entries must be absolute origins".to_string()
            })?;
            if !matches!(parsed.scheme(), "http" | "https")
                || parsed.path() != "/"
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || origin.ends_with('/')
            {
                return Err(
                    "MARKHAND_CORS_ORIGINS entries must be exact origins (scheme://host[:port])"
                        .into(),
                );
            }
            if profile == Profile::Prod && parsed.scheme() != "https" {
                return Err("prod MARKHAND_CORS_ORIGINS requires https origins".into());
            }
        }
        if self.allowed_methods.len() > MAX_CORS_LIST
            || self.allowed_headers.len() > MAX_CORS_LIST
            || self.expose_headers.len() > MAX_CORS_LIST
        {
            return Err("CORS method/header/expose allow-lists exceed configured caps".into());
        }
        for method in &self.allowed_methods {
            validate_http_method_token(method)?;
        }
        for header in &self.allowed_headers {
            validate_header_name_token(header)?;
        }
        for header in &self.expose_headers {
            validate_header_name_token(header)?;
        }
        if self.allow_credentials && self.allowed_origins.is_empty() {
            return Err(
                "MARKHAND_CORS_ALLOW_CREDENTIALS=true requires explicit MARKHAND_CORS_ORIGINS"
                    .into(),
            );
        }
        Ok(())
    }
}

fn validate_http_method_token(method: &str) -> Result<(), String> {
    if method == "*" {
        return Err("CORS wildcard methods are forbidden".into());
    }
    if method.is_empty()
        || !method
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b'-')
    {
        return Err(format!(
            "MARKHAND_CORS_METHODS entry is not a valid method token: {method}"
        ));
    }
    // Reject unknown tokens beyond the standard set used by the API.
    const ALLOWED: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
    if !ALLOWED.contains(&method) {
        return Err(format!(
            "MARKHAND_CORS_METHODS entry is not an allowed method: {method}"
        ));
    }
    Ok(())
}

fn validate_header_name_token(name: &str) -> Result<(), String> {
    if name == "*" {
        return Err("CORS wildcard headers are forbidden".into());
    }
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!(
            "CORS header allow-list entry is not a valid header token: {name}"
        ));
    }
    Ok(())
}

/// In-process fixed-window rate limit configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub window_secs: u64,
    pub max_keys: usize,
    pub exempt_health: bool,
    pub default_ip_limit: u32,
    pub default_user_limit: u32,
    pub auth_ip_limit: u32,
    pub upload_ip_limit: u32,
    pub upload_user_limit: u32,
    pub search_ip_limit: u32,
    pub search_user_limit: u32,
    pub stream_ip_limit: u32,
    pub stream_user_limit: u32,
    pub health_limit: u32,
}

impl RateLimitConfig {
    pub fn production_defaults() -> Self {
        Self {
            enabled: true,
            window_secs: DEFAULT_WINDOW_SECS,
            max_keys: DEFAULT_MAX_KEYS,
            exempt_health: true,
            default_ip_limit: DEFAULT_DEFAULT_IP_LIMIT,
            default_user_limit: DEFAULT_DEFAULT_USER_LIMIT,
            auth_ip_limit: DEFAULT_AUTH_IP_LIMIT,
            upload_ip_limit: DEFAULT_UPLOAD_IP_LIMIT,
            upload_user_limit: DEFAULT_UPLOAD_USER_LIMIT,
            search_ip_limit: DEFAULT_SEARCH_IP_LIMIT,
            search_user_limit: DEFAULT_SEARCH_USER_LIMIT,
            stream_ip_limit: DEFAULT_STREAM_IP_LIMIT,
            stream_user_limit: DEFAULT_STREAM_USER_LIMIT,
            health_limit: DEFAULT_HEALTH_LIMIT,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_for_profile(Profile::Dev)
    }

    pub fn validate_for_profile(&self, profile: Profile) -> Result<(), String> {
        if profile == Profile::Prod && !self.enabled {
            return Err("prod profile requires MARKHAND_RATE_LIMIT_ENABLED=true".into());
        }
        if self.window_secs == 0 || self.window_secs > MAX_WINDOW_SECS {
            return Err(format!(
                "MARKHAND_RATE_LIMIT_WINDOW_SECS must be between 1 and {MAX_WINDOW_SECS}"
            ));
        }
        if self.max_keys == 0 || self.max_keys > MAX_MAX_KEYS {
            return Err(format!(
                "MARKHAND_RATE_LIMIT_MAX_KEYS must be between 1 and {MAX_MAX_KEYS}"
            ));
        }
        for (name, value) in [
            ("MARKHAND_RATE_LIMIT_DEFAULT_IP", self.default_ip_limit),
            ("MARKHAND_RATE_LIMIT_DEFAULT_USER", self.default_user_limit),
            ("MARKHAND_RATE_LIMIT_AUTH_IP", self.auth_ip_limit),
            ("MARKHAND_RATE_LIMIT_UPLOAD_IP", self.upload_ip_limit),
            ("MARKHAND_RATE_LIMIT_UPLOAD_USER", self.upload_user_limit),
            ("MARKHAND_RATE_LIMIT_SEARCH_IP", self.search_ip_limit),
            ("MARKHAND_RATE_LIMIT_SEARCH_USER", self.search_user_limit),
            ("MARKHAND_RATE_LIMIT_STREAM_IP", self.stream_ip_limit),
            ("MARKHAND_RATE_LIMIT_STREAM_USER", self.stream_user_limit),
            ("MARKHAND_RATE_LIMIT_HEALTH", self.health_limit),
        ] {
            if value == 0 || value > MAX_LIMIT {
                return Err(format!("{name} must be between 1 and {MAX_LIMIT}"));
            }
        }
        Ok(())
    }
}

pub fn parse_csv_list(raw: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(trimmed.to_string());
    }
    Ok(out)
}

pub fn parse_trusted_proxies(raw: &str) -> Result<TrustedProxies, String> {
    let mut cidrs = Vec::new();
    for part in raw.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let net = IpNet::from_str(trimmed)
            .map_err(|_| format!("MARKHAND_TRUSTED_PROXY_CIDRS entry is invalid: {trimmed}"))?;
        if net.prefix_len() == 0 {
            return Err(
                "MARKHAND_TRUSTED_PROXY_CIDRS rejects /0 (or equivalent) catch-all networks".into(),
            );
        }
        cidrs.push(net);
    }
    if cidrs.len() > MAX_ORIGINS {
        return Err(format!(
            "MARKHAND_TRUSTED_PROXY_CIDRS accepts at most {MAX_ORIGINS} CIDRs"
        ));
    }
    Ok(TrustedProxies::from_cidrs(cidrs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wildcard_cors_in_any_profile() {
        let mut cors = CorsConfig::production_defaults();
        cors.allowed_origins = vec!["*".into()];
        assert!(cors
            .validate(Profile::Dev)
            .unwrap_err()
            .contains("wildcard"));
    }

    #[test]
    fn rejects_zero_rate_limits() {
        let mut cfg = RateLimitConfig::production_defaults();
        cfg.default_ip_limit = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn parses_proxy_cidrs() {
        let proxies = parse_trusted_proxies("10.0.0.0/8, 192.168.0.0/16").unwrap();
        assert!(!proxies.is_empty());
        assert!(parse_trusted_proxies("not-a-cidr").is_err());
        assert!(parse_trusted_proxies("0.0.0.0/0")
            .unwrap_err()
            .contains("/0"));
    }

    #[test]
    fn prod_requires_limiter_enabled() {
        let mut cfg = RateLimitConfig::production_defaults();
        cfg.enabled = false;
        assert!(cfg
            .validate_for_profile(Profile::Prod)
            .unwrap_err()
            .contains("RATE_LIMIT_ENABLED"));
        assert!(cfg.validate_for_profile(Profile::Dev).is_ok());
    }

    #[test]
    fn rejects_invalid_cors_method_token() {
        let mut cors = CorsConfig::production_defaults();
        cors.allowed_methods = vec!["GET".into(), "FOO".into()];
        assert!(cors.validate(Profile::Dev).is_err());
    }
}
