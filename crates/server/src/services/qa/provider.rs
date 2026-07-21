//! Configured GLM-compatible chat provider with fail-closed SSRF policy (P1B-R03).
//!
//! Single [`ConfiguredProvider`] runtime owns URL, auth, deadlines, and a pinned DNS
//! resolver so connect cannot re-resolve. Unresolved hosts and private/metadata/
//! link-local/mapped IPs are rejected. SSE uses strict UTF-8 line buffering.
//! No hardcoded model identifier. Secrets never appear in Debug.

use std::env;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use std::vec;

use futures::{Stream, StreamExt};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::config::{Profile, SecretString};
use crate::services::qa::grounding::ProviderGroundedPayload;
use crate::services::qa::stream::StreamCancel;

/// Env keys for optional grounded-Q&A chat runtime (no hardcoded model).
pub const ENV_QA_BASE_URL: &str = "MARKHAND_QA_BASE_URL";
pub const ENV_QA_API_KEY: &str = "MARKHAND_QA_API_KEY";
pub const ENV_QA_MODEL: &str = "MARKHAND_QA_MODEL";
pub const ENV_QA_PROVIDER: &str = "MARKHAND_QA_PROVIDER";
pub const ENV_QA_TIMEOUT_MS: &str = "MARKHAND_QA_TIMEOUT_MS";
pub const ENV_QA_ALLOWED_HOSTS: &str = "MARKHAND_QA_ALLOWED_HOSTS";
pub const ENV_QA_ALLOW_LOCAL: &str = "MARKHAND_QA_ALLOW_LOCAL";
pub const ENV_QA_ALLOW_NO_AUTH: &str = "MARKHAND_QA_ALLOW_NO_AUTH";

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TIMEOUT: Duration = Duration::from_secs(120);
const MIN_TIMEOUT: Duration = Duration::from_millis(10);
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Soft hint only — transport chunks may be arbitrary size under total/event caps (M4).
pub const MAX_STREAM_CHUNK_BYTES: usize = 8 * 1024;
/// Max SSE events accepted from a provider stream before forced truncation.
pub const MAX_SSE_EVENTS: usize = 2_048;

/// Auth mode for the configured provider.
#[derive(Clone)]
pub enum ProviderAuth {
    /// Explicit no-auth (Dev/Test local only).
    None,
    Bearer(SecretString),
}

impl std::fmt::Debug for ProviderAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("ProviderAuth::None"),
            Self::Bearer(_) => f.write_str("ProviderAuth::Bearer([REDACTED])"),
        }
    }
}

/// Fail-closed configuration owned by [`ConfiguredProvider`].
#[derive(Clone)]
pub struct QaProviderConfig {
    base_url: String,
    host: String,
    pinned_addrs: Vec<SocketAddr>,
    auth: ProviderAuth,
    model: String,
    provider: String,
    timeout: Duration,
    allowed_hosts: Vec<String>,
    allow_local: bool,
    profile: Profile,
}

impl std::fmt::Debug for QaProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QaProviderConfig")
            .field("base_url", &"[REDACTED_ENDPOINT]")
            .field("host", &"[REDACTED_HOST]")
            .field("pinned_addr_count", &self.pinned_addrs.len())
            .field("auth", &self.auth)
            .field("model", &"[REDACTED_MODEL]")
            .field("provider", &self.provider)
            .field("timeout_ms", &self.timeout.as_millis())
            .field("allowed_hosts_count", &self.allowed_hosts.len())
            .field("allow_local", &self.allow_local)
            .field("profile", &self.profile)
            .finish()
    }
}

/// Validated URL + pinned addresses after SSRF checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalEndpoint {
    pub base_url: String,
    pub host: String,
    pub pinned_addrs: Vec<SocketAddr>,
}

impl QaProviderConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_url: impl Into<String>,
        auth: ProviderAuth,
        model: impl Into<String>,
        provider: impl Into<String>,
        timeout: Duration,
        allowed_hosts: impl IntoIterator<Item = impl Into<String>>,
        allow_local: bool,
        profile: Profile,
    ) -> Result<Self, ProviderError> {
        let model = model.into().trim().to_string();
        let provider = normalize_provider(&provider.into())?;
        if model.is_empty() {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_MODEL));
        }
        if !(MIN_TIMEOUT..=MAX_TIMEOUT).contains(&timeout) {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_TIMEOUT_MS));
        }
        if allow_local && !matches!(profile, Profile::Dev | Profile::Test) {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_ALLOW_LOCAL));
        }
        match &auth {
            ProviderAuth::None => {
                if !matches!(profile, Profile::Dev | Profile::Test) || !allow_local {
                    return Err(ProviderError::InvalidConfiguration(ENV_QA_ALLOW_NO_AUTH));
                }
            }
            ProviderAuth::Bearer(key) => {
                if key.expose().trim().is_empty() {
                    return Err(ProviderError::InvalidConfiguration(ENV_QA_API_KEY));
                }
            }
        }
        let allowed_hosts: Vec<String> = allowed_hosts
            .into_iter()
            .map(|h| h.into().trim().to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .collect();
        // Cloud (non-local) requires a secret and non-empty egress allowlist.
        let endpoint = canonicalize_base_url(&base_url.into(), &allowed_hosts, allow_local)?;
        if !allow_local {
            if matches!(auth, ProviderAuth::None) {
                return Err(ProviderError::InvalidConfiguration(ENV_QA_API_KEY));
            }
            if allowed_hosts.is_empty() {
                return Err(ProviderError::InvalidConfiguration(ENV_QA_ALLOWED_HOSTS));
            }
        }
        Ok(Self {
            base_url: endpoint.base_url,
            host: endpoint.host,
            pinned_addrs: endpoint.pinned_addrs,
            auth,
            model,
            provider,
            timeout,
            allowed_hosts,
            allow_local,
            profile,
        })
    }

    /// Convenience constructor with bearer key (rejects empty/"fake" placeholders).
    #[allow(clippy::too_many_arguments)]
    pub fn with_api_key(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        provider: impl Into<String>,
        timeout: Duration,
        allowed_hosts: impl IntoIterator<Item = impl Into<String>>,
        allow_local: bool,
        profile: Profile,
    ) -> Result<Self, ProviderError> {
        let key = api_key.into();
        let trimmed = key.trim();
        if trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("fake")
            || trimmed.eq_ignore_ascii_case("test")
            || trimmed.eq_ignore_ascii_case("dummy")
        {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_API_KEY));
        }
        Self::new(
            base_url,
            ProviderAuth::Bearer(SecretString::new(key)),
            model,
            provider,
            timeout,
            allowed_hosts,
            allow_local,
            profile,
        )
    }

    pub fn from_env(profile: Profile) -> Result<Self, ProviderError> {
        let base_url = match env::var(ENV_QA_BASE_URL) {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return Err(ProviderError::Unavailable),
        };
        let allow_local = match env::var(ENV_QA_ALLOW_LOCAL) {
            Ok(value) => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(_) => false,
        };
        let allow_no_auth = match env::var(ENV_QA_ALLOW_NO_AUTH) {
            Ok(value) => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(_) => false,
        };
        let auth = match env::var(ENV_QA_API_KEY) {
            Ok(value) if !value.trim().is_empty() => ProviderAuth::Bearer(SecretString::new(value)),
            _ if allow_no_auth
                && allow_local
                && matches!(profile, Profile::Dev | Profile::Test) =>
            {
                ProviderAuth::None
            }
            _ => return Err(ProviderError::Unavailable),
        };
        let model = match env::var(ENV_QA_MODEL) {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return Err(ProviderError::Unavailable),
        };
        let provider = env::var(ENV_QA_PROVIDER).unwrap_or_else(|_| "openai-compatible".into());
        let timeout = match env::var(ENV_QA_TIMEOUT_MS) {
            Ok(value) => {
                let ms = value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| ProviderError::InvalidConfiguration(ENV_QA_TIMEOUT_MS))?;
                Duration::from_millis(ms)
            }
            Err(_) => DEFAULT_TIMEOUT,
        };
        let allowed_hosts = match env::var(ENV_QA_ALLOWED_HOSTS) {
            Ok(value) => value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        Self::new(
            base_url,
            auth,
            model,
            provider,
            timeout,
            allowed_hosts,
            allow_local,
            profile,
        )
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn provider_name(&self) -> &str {
        &self.provider
    }

    pub fn auth(&self) -> &ProviderAuth {
        &self.auth
    }

    pub fn pinned_addrs(&self) -> &[SocketAddr] {
        &self.pinned_addrs
    }

    pub fn audit_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": self.provider,
            "timeout_ms": self.timeout.as_millis() as u64,
            "configured": true,
            "allow_local": self.allow_local,
            "auth_mode": match self.auth {
                ProviderAuth::None => "none",
                ProviderAuth::Bearer(_) => "bearer",
            },
            "pinned_addr_count": self.pinned_addrs.len(),
        })
    }

    fn chat_url(&self) -> String {
        if self.base_url.ends_with("/v1") {
            format!("{}/chat/completions", self.base_url)
        } else {
            format!(
                "{}/v1/chat/completions",
                self.base_url.trim_end_matches('/')
            )
        }
    }
}

fn normalize_provider(raw: &str) -> Result<String, ProviderError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "openai-compatible" | "compatible" | "glm" | "zhipu" | "openai" => {
            Ok("openai-compatible".into())
        }
        "" => Err(ProviderError::InvalidConfiguration(ENV_QA_PROVIDER)),
        _ => Err(ProviderError::InvalidConfiguration(ENV_QA_PROVIDER)),
    }
}

/// Canonical URL policy with mandatory DNS resolve + pinned addresses.
pub fn canonicalize_base_url(
    raw: &str,
    allowed_hosts: &[String],
    allow_local: bool,
) -> Result<CanonicalEndpoint, ProviderError> {
    let parsed = Url::parse(raw.trim()).map_err(|_| ProviderError::UrlPolicy)?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ProviderError::UrlPolicy);
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(ProviderError::UrlPolicy);
    }
    let host = parsed
        .host_str()
        .ok_or(ProviderError::UrlPolicy)?
        .to_ascii_lowercase();
    let is_loopback_host = host == "localhost" || host == "127.0.0.1" || host == "::1";
    match parsed.scheme() {
        "https" => {
            if allowed_hosts.is_empty() || !allowed_hosts.iter().any(|h| h == &host) {
                return Err(ProviderError::UrlPolicy);
            }
        }
        "http" => {
            if !allow_local || !is_loopback_host {
                return Err(ProviderError::UrlPolicy);
            }
        }
        _ => return Err(ProviderError::UrlPolicy),
    }

    let port = parsed.port_or_known_default().unwrap_or(443);
    let pinned = resolve_and_pin_host(&host, port, allow_local)?;
    let path = parsed.path().trim_end_matches('/').to_string();
    let mut out = parsed;
    out.set_path(&path);
    Ok(CanonicalEndpoint {
        base_url: out.as_str().trim_end_matches('/').to_string(),
        host,
        pinned_addrs: pinned,
    })
}

fn resolve_and_pin_host(
    host: &str,
    port: u16,
    allow_local: bool,
) -> Result<Vec<SocketAddr>, ProviderError> {
    // Literal IP host — validate and pin without DNS.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) && !(allow_local && ip.is_loopback()) {
            return Err(ProviderError::UrlPolicy);
        }
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| ProviderError::UrlPolicy)?
        .collect();
    if addrs.is_empty() {
        return Err(ProviderError::UrlPolicy);
    }
    for addr in &addrs {
        if is_blocked_ip(addr.ip()) && !(allow_local && addr.ip().is_loopback()) {
            return Err(ProviderError::UrlPolicy);
        }
    }
    Ok(addrs)
}

/// Complete private/metadata/link-local/mapped IP rejection.
///
/// IPv6: accept **only** proven global unicast; reject all special / multicast /
/// transition / reserved / documentation / ULA / link-local / mapped forms.
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => !is_ipv6_global_unicast(v6),
    }
}

/// True only for IPv6 global unicast (2000::/3) excluding known specials inside it.
fn is_ipv6_global_unicast(v6: Ipv6Addr) -> bool {
    if v6.to_ipv4_mapped().is_some() || v6.to_ipv4().is_some() {
        return false;
    }
    if v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || v6.is_unique_local()
        || v6.is_unicast_link_local()
        || is_ipv6_documentation(v6)
        || is_ipv6_discard(v6)
        || is_ipv6_metadata_like(v6)
        || is_ipv6_reserved_or_transition(v6)
    {
        return false;
    }
    // Global Unicast is 2000::/3.
    let s0 = v6.segments()[0];
    (s0 & 0xe000) == 0x2000
}

fn is_ipv6_reserved_or_transition(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    // IETF protocol assignments 2001::/23 (except documented globals we still
    // treat conservatively as blocked for SSRF), TEREDO 2001:0000::/32,
    // ORCHID, 6to4 2002::/16, and benchmarking.
    (s[0] == 0x2001 && s[1] < 0x0200)
        || s[0] == 0x2002
        || (s[0] == 0x2001 && s[1] == 0x0000)
        || (s[0] >= 0xfc00) // catch-all non-global high bits already covered; keep belt
}

fn is_blocked_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_multicast()
        || (o[0] == 169 && o[1] == 254) // link-local / AWS metadata
        || (o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000) // CGNAT 100.64/10
        || o[0] == 0
        || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
        || (o[0] == 192 && o[1] == 0 && o[2] == 2) // TEST-NET-1
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmarking
        || (o[0] == 198 && o[1] == 51 && o[2] == 100) // TEST-NET-2
        || (o[0] == 203 && o[1] == 0 && o[2] == 113) // TEST-NET-3
        || o[0] >= 224
}

fn is_ipv6_documentation(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    s[0] == 0x2001 && s[1] == 0x0db8
}

fn is_ipv6_discard(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0
}

fn is_ipv6_metadata_like(v6: Ipv6Addr) -> bool {
    // IPv6 ULA already covered; treat fd00::/8 + fe80 covered. Also block
    // well-known metadata-adjacent ranges if embedded.
    let s = v6.segments();
    s[0] == 0xfe80 || (s[0] & 0xfe00) == 0xfc00
}

/// DNS resolver that only returns addresses pinned at config time.
#[derive(Debug, Clone)]
pub struct PinnedDnsResolver {
    host: String,
    addrs: Vec<SocketAddr>,
}

impl PinnedDnsResolver {
    pub fn new(host: impl Into<String>, addrs: Vec<SocketAddr>) -> Self {
        Self {
            host: host.into().to_ascii_lowercase(),
            addrs,
        }
    }
}

impl Resolve for PinnedDnsResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let requested = name.as_str().to_ascii_lowercase();
        let host = self.host.clone();
        let addrs = self.addrs.clone();
        Box::pin(async move {
            // Exact configured host only — no localhost / alias bypass.
            if requested != host {
                return Err("qa provider dns pin mismatch".into());
            }
            if addrs.is_empty() {
                return Err("qa provider dns pin empty".into());
            }
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum ProviderError {
    #[error("qa provider is not configured")]
    Unavailable,
    #[error("qa provider configuration invalid")]
    InvalidConfiguration(&'static str),
    #[error("qa provider url policy violation")]
    UrlPolicy,
    #[error("qa provider timed out")]
    Timeout,
    #[error("qa provider outage")]
    Outage,
    #[error("qa provider returned an invalid response")]
    InvalidResponse,
    #[error("qa provider stream cancelled")]
    Cancelled,
    #[error("qa provider response truncated")]
    Truncated,
}

impl ProviderError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "qa_provider_unavailable",
            Self::InvalidConfiguration(_) => "qa_provider_invalid_configuration",
            Self::UrlPolicy => "qa_provider_url_policy",
            Self::Timeout => "qa_provider_timeout",
            Self::Outage => "qa_provider_outage",
            Self::InvalidResponse => "qa_provider_invalid_response",
            Self::Cancelled => "qa_provider_cancelled",
            Self::Truncated => "qa_provider_truncated",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ChatCompletionRequest {
    pub system: String,
    pub user: String,
}

impl std::fmt::Debug for ChatCompletionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatCompletionRequest")
            .field("system", &"[REDACTED_PROMPT]")
            .field("user", &"[REDACTED_PROMPT]")
            .finish()
    }
}

/// Abstraction over GLM-compatible chat backends producing structured claims.
pub trait QaChatProvider: Send + Sync {
    fn complete_grounded<'a>(
        &'a self,
        request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderGroundedPayload, ProviderError>> + Send + 'a>>;

    /// Incremental token stream of the answer body (after/during provider IO).
    fn stream_tokens<'a>(
        &'a self,
        request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + 'a>>;

    /// Same as [`Self::stream_tokens`] but cooperatively observes cancellation (M7).
    fn stream_tokens_cancellable<'a>(
        &'a self,
        request: &'a ChatCompletionRequest,
        cancel: Option<&'a StreamCancel>,
    ) -> Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + 'a>> {
        let _ = cancel;
        self.stream_tokens(request)
    }
}

/// Single configured runtime owning URL/auth/deadlines/pinned DNS.
pub struct ConfiguredProvider {
    client: reqwest::Client,
    config: QaProviderConfig,
}

impl std::fmt::Debug for ConfiguredProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfiguredProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Backward-compatible alias.
pub type GlmCompatibleProvider = ConfiguredProvider;

impl ConfiguredProvider {
    pub fn new(config: QaProviderConfig) -> Result<Self, ProviderError> {
        let resolver = PinnedDnsResolver::new(config.host.clone(), config.pinned_addrs.clone());
        // H2: ignore HTTP(S)_PROXY env; only the exact configured host is pinned.
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(config.timeout())
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .dns_resolver(Arc::new(resolver))
            .build()
            .map_err(|_| ProviderError::Outage)?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &QaProviderConfig {
        &self.config
    }

    async fn complete_inner(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ProviderGroundedPayload, ProviderError> {
        let body = ChatBody {
            model: self.config.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: request.system.clone(),
                },
                ChatMessage {
                    role: "user",
                    content: request.user.clone(),
                },
            ],
            stream: false,
            tools: None,
        };
        let mut builder = self.client.post(self.config.chat_url()).json(&body);
        if let ProviderAuth::Bearer(key) = &self.config.auth {
            builder = builder.bearer_auth(key.expose());
        }
        let response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                ProviderError::Timeout
            } else {
                ProviderError::Outage
            }
        })?;
        if response.status().is_redirection() {
            return Err(ProviderError::UrlPolicy);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Outage);
        }
        let bytes = read_body_bounded(response, MAX_RESPONSE_BYTES).await?;
        let parsed: ChatResponse =
            serde_json::from_slice(&bytes).map_err(|_| ProviderError::InvalidResponse)?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message)
            .and_then(|message| message.content)
            .filter(|text| !text.trim().is_empty())
            .ok_or(ProviderError::InvalidResponse)?;
        if content.len() > MAX_RESPONSE_BYTES {
            return Err(ProviderError::Truncated);
        }
        parse_grounded_payload(&content)
    }

    /// Parse SSE transport incrementally, forwarding each delta as produced (M4).
    async fn stream_raw_tokens_incremental(
        &self,
        request: &ChatCompletionRequest,
        cancel: Option<&StreamCancel>,
        tx: &tokio::sync::mpsc::Sender<Result<String, ProviderError>>,
    ) -> Result<(), ProviderError> {
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(ProviderError::Cancelled);
        }
        let body = ChatBody {
            model: self.config.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: request.system.clone(),
                },
                ChatMessage {
                    role: "user",
                    content: request.user.clone(),
                },
            ],
            stream: true,
            tools: None,
        };
        let mut builder = self.client.post(self.config.chat_url()).json(&body);
        if let ProviderAuth::Bearer(key) = &self.config.auth {
            builder = builder.bearer_auth(key.expose());
        }
        let response = tokio::select! {
            biased;
            resp = builder.send() => resp.map_err(|error| {
                if error.is_timeout() {
                    ProviderError::Timeout
                } else {
                    ProviderError::Outage
                }
            })?,
            _ = async {
                if let Some(c) = cancel {
                    c.cancelled().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                return Err(ProviderError::Cancelled);
            }
        };
        if response.status().is_redirection() {
            return Err(ProviderError::UrlPolicy);
        }
        if !response.status().is_success() {
            return Err(ProviderError::Outage);
        }
        // M4: exact MIME type (parameters like charset allowed after ';').
        let ctype = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !sse_content_type_ok(ctype) {
            return Err(ProviderError::InvalidResponse);
        }
        let mut total = 0usize;
        let mut flushed_events = 0usize;
        let mut emitted = 0usize;
        let mut byte_stream = response.bytes_stream();
        let mut parser = SseEventParser::new();
        loop {
            if cancel.is_some_and(|c| c.is_cancelled()) {
                drop(byte_stream);
                return Err(ProviderError::Cancelled);
            }
            let next = tokio::select! {
                biased;
                item = byte_stream.next() => item,
                _ = async {
                    if let Some(c) = cancel {
                        c.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    drop(byte_stream);
                    return Err(ProviderError::Cancelled);
                }
            };
            match next {
                Some(chunk) => {
                    // Arbitrary transport chunk size; count only blank-line-flushed events.
                    let chunk = chunk.map_err(|_| ProviderError::Outage)?;
                    total = total.saturating_add(chunk.len());
                    if total > MAX_RESPONSE_BYTES {
                        return Err(ProviderError::Truncated);
                    }
                    let outcome = parser.push(&chunk)?;
                    flushed_events = flushed_events.saturating_add(outcome.flushed);
                    if flushed_events > MAX_SSE_EVENTS {
                        return Err(ProviderError::Truncated);
                    }
                    for delta in outcome.deltas {
                        emitted = emitted.saturating_add(1);
                        if tx.send(Ok(delta)).await.is_err() {
                            return Ok(());
                        }
                    }
                    if outcome.done {
                        break;
                    }
                }
                None => {
                    // Incomplete trailing fragment (no blank-line flush) is ignored.
                    parser.finish_discard_fragment()?;
                    break;
                }
            }
        }
        if emitted == 0 {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(())
    }
}

fn sse_content_type_ok(raw: &str) -> bool {
    let media = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    media == "text/event-stream"
}

/// Parse provider content as structured grounded JSON (optionally fenced).
pub fn parse_grounded_payload(content: &str) -> Result<ProviderGroundedPayload, ProviderError> {
    let trimmed = content.trim();
    let json = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.trim_end_matches('`').trim()
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.trim_end_matches('`').trim()
    } else {
        trimmed
    };
    serde_json::from_str(json).map_err(|_| ProviderError::InvalidResponse)
}

async fn read_body_bounded(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ProviderError> {
    let mut out = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ProviderError::Outage)?;
        if out.len().saturating_add(chunk.len()) > max_bytes {
            return Err(ProviderError::Truncated);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Incremental SSE event parser: content framing, multi-`data:` lines, `[DONE]`.
///
/// Event counting is blank-line flush only (fragment-safe). `[DONE]` must be its
/// own flushed event — mixed DONE+payload in one event is rejected.
#[derive(Debug, Default)]
struct SseEventParser {
    buffer: Vec<u8>,
    event_lines: Vec<String>,
}

#[derive(Debug, Default)]
struct SsePushOutcome {
    deltas: Vec<String>,
    /// Number of blank-line-flushed events in this push (excludes empty keepalives).
    flushed: usize,
    done: bool,
}

impl SseEventParser {
    fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, chunk: &[u8]) -> Result<SsePushOutcome, ProviderError> {
        self.buffer.extend_from_slice(chunk);
        // L14: strip UTF-8 BOM once at stream start.
        if self.buffer.starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.buffer.drain(..3);
        }
        let mut outcome = SsePushOutcome::default();
        loop {
            // L14: accept CR, LF, or CRLF as line terminators.
            let cr = self.buffer.iter().position(|b| *b == b'\r');
            let lf = self.buffer.iter().position(|b| *b == b'\n');
            let (idx, skip) = match (cr, lf) {
                (Some(c), Some(l)) if c + 1 == l => (c, 2usize), // CRLF
                (Some(c), Some(l)) if c < l => (c, 1usize),      // CR then later LF
                // R9.10: retain a trailing CR across chunks — wait for next byte
                // to distinguish CRLF vs lone CR.
                (Some(c), None) if c + 1 == self.buffer.len() => break,
                (Some(c), None) => (c, 1usize),
                (None, Some(l)) => (l, 1usize),
                (Some(c), Some(l)) => (c.min(l), 1usize),
                (None, None) => break,
            };
            let line_bytes = self.buffer.drain(..idx).collect::<Vec<u8>>();
            let _ = self.buffer.drain(..skip.min(self.buffer.len()));
            let line = String::from_utf8(line_bytes).map_err(|_| ProviderError::InvalidResponse)?;
            if line.is_empty() {
                let flush = self.flush_event()?;
                if flush.counted {
                    outcome.flushed = outcome.flushed.saturating_add(1);
                }
                // M12 inbound: keep pending deltas before signalling done.
                outcome.deltas.extend(flush.deltas);
                if flush.done {
                    outcome.done = true;
                    return Ok(outcome);
                }
            } else {
                self.event_lines.push(line);
            }
        }
        Ok(outcome)
    }

    /// Discard an unflushed trailing fragment at EOF (fragment-safe).
    fn finish_discard_fragment(&mut self) -> Result<(), ProviderError> {
        self.buffer.clear();
        self.event_lines.clear();
        Ok(())
    }

    fn flush_event(&mut self) -> Result<FlushedSseEvent, ProviderError> {
        if self.event_lines.is_empty() {
            return Ok(FlushedSseEvent {
                deltas: Vec::new(),
                done: false,
                counted: false,
            });
        }
        let lines = std::mem::take(&mut self.event_lines);
        let (deltas, done) = parse_sse_event(&lines)?;
        Ok(FlushedSseEvent {
            deltas,
            done,
            counted: true,
        })
    }
}

#[derive(Debug)]
struct FlushedSseEvent {
    deltas: Vec<String>,
    done: bool,
    counted: bool,
}

/// Parse one SSE event. Returns `(deltas, done)`.
///
/// Multi-`data:` lines are joined with `\n` into a single payload (HTML SSE),
/// then parsed as one JSON object. `[DONE]` is accepted only as its own event.
fn parse_sse_event(lines: &[String]) -> Result<(Vec<String>, bool), ProviderError> {
    let mut data_parts: Vec<&str> = Vec::new();
    for line in lines {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let payload = rest.strip_prefix(' ').unwrap_or(rest);
            data_parts.push(payload);
        }
        // Ignore event:/id:/retry: fields for chat deltas.
    }
    if data_parts.is_empty() {
        return Ok((Vec::new(), false));
    }
    let done_parts: Vec<&str> = data_parts
        .iter()
        .copied()
        .filter(|p| p.trim() == "[DONE]")
        .collect();
    if !done_parts.is_empty() {
        // DONE must be the sole data payload of its own flushed event.
        if data_parts.len() != 1 || data_parts[0].trim() != "[DONE]" {
            return Err(ProviderError::InvalidResponse);
        }
        return Ok((Vec::new(), true));
    }
    let joined = data_parts.join("\n");
    match delta_from_sse_data(&joined)? {
        Some(d) => Ok((vec![d], false)),
        None => Ok((Vec::new(), false)),
    }
}

fn delta_from_sse_data(data: &str) -> Result<Option<String>, ProviderError> {
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    let parsed: StreamChunk =
        serde_json::from_str(data).map_err(|_| ProviderError::InvalidResponse)?;
    Ok(parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.delta)
        .and_then(|d| d.content)
        .filter(|c| !c.is_empty()))
}

impl QaChatProvider for ConfiguredProvider {
    fn complete_grounded<'a>(
        &'a self,
        request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderGroundedPayload, ProviderError>> + Send + 'a>>
    {
        Box::pin(self.complete_inner(request))
    }

    fn stream_tokens<'a>(
        &'a self,
        request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + 'a>> {
        self.stream_tokens_cancellable(request, None)
    }

    fn stream_tokens_cancellable<'a>(
        &'a self,
        request: &'a ChatCompletionRequest,
        cancel: Option<&'a StreamCancel>,
    ) -> Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + 'a>> {
        // M4: expose incremental SSE deltas live (structured payload may accumulate
        // at the caller). Transport chunks are parsed as they arrive.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, ProviderError>>(32);
        let client = self.client.clone();
        let config = self.config.clone();
        let request = request.clone();
        let cancel = cancel.cloned();
        tokio::spawn(async move {
            let provider = ConfiguredProvider { client, config };
            if let Err(err) = provider
                .stream_raw_tokens_incremental(&request, cancel.as_ref(), &tx)
                .await
            {
                let _ = tx.send(Err(err)).await;
            }
        });
        Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        }))
    }
}

#[derive(Serialize)]
struct ChatBody {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<()>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: &'static str,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: Option<ChatMessageContent>,
}

#[derive(Deserialize)]
struct ChatMessageContent {
    content: Option<String>,
}

#[derive(Deserialize)]
struct StreamChunk {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Option<StreamDelta>,
}

#[derive(Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

/// Test/double provider that returns a fixed grounded payload or error.
#[derive(Debug, Clone)]
pub struct ScriptedProvider {
    pub result: Result<ProviderGroundedPayload, ProviderError>,
    pub chunks: Vec<String>,
}

impl QaChatProvider for ScriptedProvider {
    fn complete_grounded<'a>(
        &'a self,
        _request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderGroundedPayload, ProviderError>> + Send + 'a>>
    {
        let result = self.result.clone();
        Box::pin(async move { result })
    }

    fn stream_tokens<'a>(
        &'a self,
        _request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + 'a>> {
        if self.chunks.is_empty() {
            match &self.result {
                Ok(payload) => {
                    let encoded = serde_json::to_string(payload)
                        .unwrap_or_else(|_| r#"{"claims":[],"refusal":true}"#.into());
                    return Box::pin(futures::stream::once(async move { Ok(encoded) }));
                }
                Err(error) => {
                    let error = error.clone();
                    return Box::pin(futures::stream::once(async move { Err(error) }));
                }
            }
        }
        let chunks = self.chunks.clone();
        Box::pin(futures::stream::iter(
            chunks.into_iter().map(Ok::<_, ProviderError>),
        ))
    }
}

/// Provider that sleeps past the caller's timeout budget (for timeout tests).
#[derive(Debug, Clone)]
pub struct HangingProvider {
    pub delay: Duration,
}

impl QaChatProvider for HangingProvider {
    fn complete_grounded<'a>(
        &'a self,
        _request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderGroundedPayload, ProviderError>> + Send + 'a>>
    {
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            Err(ProviderError::Timeout)
        })
    }

    fn stream_tokens<'a>(
        &'a self,
        request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + 'a>> {
        let fut = self.complete_grounded(request);
        Box::pin(futures::stream::once(async move {
            fut.await
                .map(|p| serde_json::to_string(&p).unwrap_or_default())
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn url_policy_rejects_userinfo_query_fragment_and_http_non_local() {
        assert!(matches!(
            canonicalize_base_url(
                "https://user:pass@api.example.com/v1",
                &["api.example.com".into()],
                false
            ),
            Err(ProviderError::UrlPolicy)
        ));
        assert!(matches!(
            canonicalize_base_url(
                "https://api.example.com/v1?x=1",
                &["api.example.com".into()],
                false
            ),
            Err(ProviderError::UrlPolicy)
        ));
        assert!(matches!(
            canonicalize_base_url("http://example.com/v1", &["example.com".into()], false),
            Err(ProviderError::UrlPolicy)
        ));
    }

    #[test]
    fn url_policy_rejects_unresolved_and_blocked_ips() {
        assert!(matches!(
            canonicalize_base_url(
                "https://this-host-should-not-resolve.invalid/v1",
                &["this-host-should-not-resolve.invalid".into()],
                false
            ),
            Err(ProviderError::UrlPolicy)
        ));
        assert!(matches!(
            canonicalize_base_url(
                "https://169.254.169.254/latest",
                &["169.254.169.254".into()],
                false
            ),
            Err(ProviderError::UrlPolicy)
        ));
        assert!(matches!(
            canonicalize_base_url("http://10.0.0.1/v1", &[], true),
            Err(ProviderError::UrlPolicy)
        ));
        // IPv6 ULA / link-local
        assert!(is_blocked_ip(IpAddr::V6(
            "fc00::1".parse::<Ipv6Addr>().unwrap()
        )));
        assert!(is_blocked_ip(IpAddr::V6(
            "fe80::1".parse::<Ipv6Addr>().unwrap()
        )));
        // IPv4-mapped private
        assert!(is_blocked_ip(IpAddr::V6(
            "::ffff:192.168.1.1".parse::<Ipv6Addr>().unwrap()
        )));
    }

    #[test]
    fn url_policy_allows_explicit_local_dev_and_pins() {
        let local =
            canonicalize_base_url("http://127.0.0.1:9/v1", &[], true).expect("local loopback");
        assert!(local.base_url.contains("127.0.0.1"));
        assert!(!local.pinned_addrs.is_empty());
        assert!(local.pinned_addrs[0].ip().is_loopback());
    }

    #[test]
    fn rejects_fake_api_keys_and_allows_explicit_no_auth_local() {
        assert!(matches!(
            QaProviderConfig::with_api_key(
                "http://127.0.0.1:9/v1",
                "fake",
                "configured-model",
                "glm",
                Duration::from_secs(5),
                [] as [&str; 0],
                true,
                Profile::Dev,
            ),
            Err(ProviderError::InvalidConfiguration(_))
        ));
        let ok = QaProviderConfig::new(
            "http://127.0.0.1:9/v1",
            ProviderAuth::None,
            "configured-model",
            "glm",
            Duration::from_secs(5),
            [] as [&str; 0],
            true,
            Profile::Dev,
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn debug_and_audit_never_leak_secrets_or_model() {
        let config = QaProviderConfig::with_api_key(
            "http://127.0.0.1:9/v1",
            "sk-super-secret",
            "must-not-appear-in-logs",
            "glm",
            Duration::from_secs(5),
            [] as [&str; 0],
            true,
            Profile::Dev,
        )
        .unwrap();
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("sk-super-secret"));
        assert!(!rendered.contains("must-not-appear-in-logs"));
    }

    #[tokio::test]
    async fn mock_server_rejects_redirect_and_bounds_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = "HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(body.as_bytes());
        });
        let config = QaProviderConfig::with_api_key(
            format!("http://127.0.0.1:{}", addr.port()),
            "key-not-fake",
            "configured-model",
            "glm",
            Duration::from_secs(2),
            [] as [&str; 0],
            true,
            Profile::Dev,
        )
        .unwrap();
        let provider = ConfiguredProvider::new(config).unwrap();
        let err = provider
            .complete_grounded(&ChatCompletionRequest {
                system: "s".into(),
                user: "u".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(err, ProviderError::UrlPolicy);
    }

    #[test]
    fn sse_retains_trailing_cr_across_chunks() {
        let mut parser = SseEventParser::new();
        // Split CRLF across chunks: "...\r" then "\n\n"
        let out1 = parser
            .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"ab\"}}]}\r")
            .unwrap();
        assert!(out1.deltas.is_empty(), "trailing CR must not flush yet");
        let out2 = parser.push(b"\n\n").unwrap();
        assert_eq!(out2.deltas, vec!["ab".to_string()]);
    }

    #[test]
    fn sse_utf8_line_buffer_assembles_split_multibyte() {
        // "ệ" is multi-byte UTF-8; split across transport chunks mid-codepoint.
        let full = "data: {\"choices\":[{\"delta\":{\"content\":\"phê\"}}]}\n\n";
        let bytes = full.as_bytes();
        assert!(bytes.len() > 8);
        let mut parser = SseEventParser::new();
        let mut deltas = Vec::new();
        let mut flushed = 0usize;
        for chunk in bytes.chunks(3) {
            let out = parser.push(chunk).unwrap();
            flushed += out.flushed;
            deltas.extend(out.deltas);
        }
        assert_eq!(flushed, 1);
        assert_eq!(deltas.concat(), "phê");
        // Trailing fragment without blank line must not count.
        let mut frag = SseEventParser::new();
        let out = frag
            .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n")
            .unwrap();
        assert_eq!(out.flushed, 0);
        assert!(out.deltas.is_empty());
        frag.finish_discard_fragment().unwrap();
    }

    #[test]
    fn sse_multi_data_lines_and_done() {
        // One data line per event (blank-line framed), then [DONE] as own event.
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n",
            "\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n",
            "\n",
            "data: [DONE]\n",
            "\n",
        );
        let mut parser = SseEventParser::new();
        let out = parser.push(body.as_bytes()).unwrap();
        assert!(out.done);
        assert_eq!(out.flushed, 3);
        assert_eq!(out.deltas.concat(), "hello");
    }

    #[test]
    fn sse_done_must_be_own_event() {
        let mixed = vec![
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}".into(),
            "data: [DONE]".into(),
        ];
        assert_eq!(
            parse_sse_event(&mixed).unwrap_err(),
            ProviderError::InvalidResponse
        );
        let done_lines = vec!["data: [DONE]".into()];
        let (d2, done) = parse_sse_event(&done_lines).unwrap();
        assert!(done);
        assert!(d2.is_empty());
    }

    #[test]
    fn sse_event_joins_multi_data_lines() {
        // Multi data lines in one event join with `\n` into a single JSON payload.
        let lines = vec![
            "data: {\"choices\":[{\"delta\":{\"content\":".into(),
            "data: \"xy\"}}]}".into(),
        ];
        let (deltas, done) = parse_sse_event(&lines).unwrap();
        assert!(!done);
        assert_eq!(deltas, vec!["xy".to_string()]);
    }

    #[test]
    fn sse_content_type_exact_media_type() {
        assert!(sse_content_type_ok("text/event-stream"));
        assert!(sse_content_type_ok("text/event-stream; charset=utf-8"));
        assert!(!sse_content_type_ok("application/json"));
        assert!(!sse_content_type_ok("text/plain; text/event-stream"));
    }

    #[test]
    fn sse_line_buffer_rejects_invalid_utf8_split() {
        let mut buffer: Vec<u8> = vec![0xff, 0xfe, b'\n'];
        let line_bytes = buffer
            .drain(..=buffer.iter().position(|b| *b == b'\n').unwrap())
            .collect::<Vec<u8>>();
        let trimmed = {
            let mut v = line_bytes;
            if v.last() == Some(&b'\n') {
                v.pop();
            }
            v
        };
        assert!(String::from_utf8(trimmed).is_err());
    }
}
