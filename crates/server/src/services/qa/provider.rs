//! Configured GLM-compatible chat provider for grounded Q&A (P1B-R03).
//!
//! Bounds request/response size and wall-clock timeout. Secrets and model IDs are
//! never hardcoded and never appear in Debug. Cloud endpoints must be HTTPS on an
//! explicit host allowlist; loopback HTTP is allowed only for Dev/Test. Redirects
//! and proxy env are disabled.

use std::env;
use std::future::Future;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::config::{Profile, SecretString};
use crate::services::qa::grounding::ProviderGroundedPayload;

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
pub const MAX_REQUEST_BYTES: usize = 128 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_MODEL_CHARS: usize = 256;
pub const MAX_API_KEY_CHARS: usize = 8 * 1024;
pub const MAX_BASE_URL_CHARS: usize = 2 * 1024;
const MAX_PROVIDER_NAME_CHARS: usize = 64;
pub const MAX_ALLOWED_HOSTS: usize = 32;
pub const MAX_ALLOWED_HOST_CHARS: usize = 253;
pub const MAX_ALLOWED_HOSTS_TOTAL_BYTES: usize = 4 * 1024;

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
    auth: ProviderAuth,
    model: String,
    provider: String,
    timeout: Duration,
    allow_local: bool,
    profile: Profile,
}

impl std::fmt::Debug for QaProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QaProviderConfig")
            .field("base_url", &"[REDACTED_ENDPOINT]")
            .field("host", &"[REDACTED_HOST]")
            .field("auth", &self.auth)
            .field("model", &"[REDACTED_MODEL]")
            .field("provider", &self.provider)
            .field("timeout_ms", &self.timeout.as_millis())
            .field("allow_local", &self.allow_local)
            .field("profile", &self.profile)
            .finish()
    }
}

/// Validated URL after SSRF checks.
#[derive(Clone, PartialEq, Eq)]
pub struct CanonicalEndpoint {
    pub base_url: String,
    pub host: String,
}

impl std::fmt::Debug for CanonicalEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanonicalEndpoint")
            .field("base_url", &"[REDACTED_ENDPOINT]")
            .field("host", &"[REDACTED_HOST]")
            .finish()
    }
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
        let provider_raw = provider.into();
        if provider_raw.chars().count() > MAX_PROVIDER_NAME_CHARS {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_PROVIDER));
        }
        let provider = normalize_provider(&provider_raw)?;
        if model.is_empty() || model.chars().count() > MAX_MODEL_CHARS {
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
                let exposed = key.expose();
                if exposed.trim().is_empty() || exposed.chars().count() > MAX_API_KEY_CHARS {
                    return Err(ProviderError::InvalidConfiguration(ENV_QA_API_KEY));
                }
            }
        }
        let base_url = base_url.into();
        if base_url.trim().is_empty() || base_url.chars().count() > MAX_BASE_URL_CHARS {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_BASE_URL));
        }
        let allowed_hosts = validate_allowed_hosts(allowed_hosts)?;
        let endpoint = canonicalize_base_url(&base_url, &allowed_hosts, allow_local)?;
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
            auth,
            model,
            provider,
            timeout,
            allow_local,
            profile,
        })
    }

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
        if key.trim().is_empty() || key.chars().count() > MAX_API_KEY_CHARS {
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

    pub fn host(&self) -> &str {
        &self.host
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

/// Validate allowlist count / per-host length / total bytes before collect+DNS scan.
fn validate_allowed_hosts(
    allowed_hosts: impl IntoIterator<Item = impl Into<String>>,
) -> Result<Vec<String>, ProviderError> {
    let mut out = Vec::new();
    let mut total_bytes = 0usize;
    for host in allowed_hosts {
        let host = host.into().trim().to_ascii_lowercase();
        if host.is_empty() {
            continue;
        }
        if out.len() >= MAX_ALLOWED_HOSTS {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_ALLOWED_HOSTS));
        }
        if host.len() > MAX_ALLOWED_HOST_CHARS {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_ALLOWED_HOSTS));
        }
        total_bytes = total_bytes.saturating_add(host.len());
        if total_bytes > MAX_ALLOWED_HOSTS_TOTAL_BYTES {
            return Err(ProviderError::InvalidConfiguration(ENV_QA_ALLOWED_HOSTS));
        }
        out.push(host);
    }
    Ok(out)
}

/// Canonical URL policy: HTTPS + allowlist, or explicit local HTTP loopback.
pub fn canonicalize_base_url(
    raw: &str,
    allowed_hosts: &[String],
    allow_local: bool,
) -> Result<CanonicalEndpoint, ProviderError> {
    if raw.trim().is_empty() || raw.chars().count() > MAX_BASE_URL_CHARS {
        return Err(ProviderError::UrlPolicy);
    }
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
    // Reject DNS that resolves only to blocked non-loopback addresses when local.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) && !(allow_local && ip.is_loopback()) {
            return Err(ProviderError::UrlPolicy);
        }
    } else if !is_loopback_host {
        let addrs: Vec<SocketAddr> = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|_| ProviderError::UrlPolicy)?
            .collect();
        if addrs.is_empty() {
            return Err(ProviderError::UrlPolicy);
        }
        for addr in &addrs {
            if is_blocked_ip(addr.ip()) {
                return Err(ProviderError::UrlPolicy);
            }
        }
    }

    let path = parsed.path().trim_end_matches('/').to_string();
    let mut out = parsed;
    out.set_path(&path);
    Ok(CanonicalEndpoint {
        base_url: out.as_str().trim_end_matches('/').to_string(),
        host,
    })
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || (o[0] == 169 && o[1] == 254)
                || (o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000)
                || o[0] == 0
                || o[0] >= 224
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.to_ipv4_mapped().is_some()
        }
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
}

/// Single configured runtime owning URL/auth/deadlines (no redirects, no proxy).
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

impl ConfiguredProvider {
    pub fn new(config: QaProviderConfig) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| ProviderError::Outage)?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &QaProviderConfig {
        &self.config
    }
}

#[derive(Serialize)]
struct ChatRequestBody<'a> {
    model: &'a str,
    temperature: f32,
    messages: [ChatMessage<'a>; 2],
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponseBody {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

/// Serialize the chat body and enforce the hard request byte cap (includes model).
pub fn serialize_chat_request(
    model: &str,
    request: &ChatCompletionRequest,
) -> Result<Vec<u8>, ProviderError> {
    let body = ChatRequestBody {
        model,
        temperature: 0.0,
        messages: [
            ChatMessage {
                role: "system",
                content: &request.system,
            },
            ChatMessage {
                role: "user",
                content: &request.user,
            },
        ],
    };
    let bytes = serde_json::to_vec(&body).map_err(|_| ProviderError::InvalidResponse)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ProviderError::Truncated);
    }
    Ok(bytes)
}

/// Read a provider response body with a hard byte cap before full allocation.
pub async fn read_response_capped(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ProviderError> {
    if let Some(len) = response.content_length() {
        if len > max_bytes as u64 {
            return Err(ProviderError::Truncated);
        }
    }
    let mut out = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(next) = stream.next().await {
        let chunk = next.map_err(|_| ProviderError::InvalidResponse)?;
        let next_len = out.len().saturating_add(chunk.len());
        if next_len > max_bytes {
            return Err(ProviderError::Truncated);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

impl QaChatProvider for ConfiguredProvider {
    fn complete_grounded<'a>(
        &'a self,
        request: &'a ChatCompletionRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderGroundedPayload, ProviderError>> + Send + 'a>>
    {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let result = async {
                let body = serialize_chat_request(&self.config.model, request)?;
                let mut builder = self
                    .client
                    .post(self.config.chat_url())
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(body);
                if let ProviderAuth::Bearer(key) = &self.config.auth {
                    builder = builder.bearer_auth(key.expose());
                }
                let response = builder.send().await.map_err(|_| ProviderError::Outage)?;
                if response.status().is_redirection() {
                    return Err(ProviderError::UrlPolicy);
                }
                if !response.status().is_success() {
                    return Err(ProviderError::Outage);
                }
                let bytes = read_response_capped(response, MAX_RESPONSE_BYTES).await?;
                parse_grounded_payload(&bytes)
            }
            .await;
            let outcome = match &result {
                Ok(_) => "success",
                Err(ProviderError::Outage) => "outage",
                Err(ProviderError::Timeout) => "timeout",
                Err(ProviderError::Truncated) => "truncated",
                Err(_) => "error",
            };
            crate::telemetry::record_retrieval("qa_provider", outcome, started.elapsed());
            // Error classes only — never log prompt/answer bodies.
            tracing::info!(
                target: "qa_provider",
                result = outcome,
                duration_ms = started.elapsed().as_millis() as u64,
                "qa provider complete"
            );
            result
        })
    }
}

/// Parse provider JSON into structured claims (OpenAI message content or bare payload).
pub fn parse_grounded_payload(bytes: &[u8]) -> Result<ProviderGroundedPayload, ProviderError> {
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ProviderError::Truncated);
    }
    if let Ok(direct) = serde_json::from_slice::<ProviderGroundedPayload>(bytes) {
        return Ok(direct);
    }
    let envelope: ChatResponseBody =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::InvalidResponse)?;
    let content = envelope
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .ok_or(ProviderError::InvalidResponse)?;
    let trimmed = content.trim();
    if trimmed.len() > MAX_RESPONSE_BYTES {
        return Err(ProviderError::Truncated);
    }
    serde_json::from_str(trimmed).map_err(|_| ProviderError::InvalidResponse)
}

/// Deterministic in-memory provider for hermetic tests.
pub struct ScriptedProvider {
    pub result: Result<ProviderGroundedPayload, ProviderError>,
}

impl std::fmt::Debug for ScriptedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedProvider")
            .field(
                "result",
                &self.result.as_ref().map(|_| "[REDACTED_PAYLOAD]"),
            )
            .finish()
    }
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
}

/// Provider that never completes within the configured budget (timeout tests).
pub struct HangingProvider {
    pub delay: Duration,
}

impl std::fmt::Debug for HangingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HangingProvider")
            .field("delay_ms", &self.delay.as_millis())
            .finish()
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::qa::grounding::StructuredClaim;

    #[test]
    fn debug_redacts_secrets_and_model() {
        let config = QaProviderConfig::with_api_key(
            "http://127.0.0.1:9/v1",
            "super-secret-key",
            "secret-model-id",
            "glm",
            Duration::from_secs(5),
            [] as [&str; 0],
            true,
            Profile::Dev,
        )
        .unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("super-secret-key"));
        assert!(!debug.contains("secret-model-id"));
        assert!(debug.contains("[REDACTED]"));
        let endpoint = canonicalize_base_url("http://127.0.0.1:9/v1", &[], true).unwrap();
        assert!(!format!("{endpoint:?}").contains("127.0.0.1"));
    }

    #[test]
    fn rejects_oversized_model_key_and_url() {
        let long_model = "m".repeat(MAX_MODEL_CHARS + 1);
        assert!(matches!(
            QaProviderConfig::with_api_key(
                "http://127.0.0.1:9/v1",
                "key",
                long_model,
                "glm",
                Duration::from_secs(5),
                [] as [&str; 0],
                true,
                Profile::Dev,
            ),
            Err(ProviderError::InvalidConfiguration(ENV_QA_MODEL))
        ));
        let long_key = "k".repeat(MAX_API_KEY_CHARS + 1);
        assert!(matches!(
            QaProviderConfig::with_api_key(
                "http://127.0.0.1:9/v1",
                long_key,
                "model",
                "glm",
                Duration::from_secs(5),
                [] as [&str; 0],
                true,
                Profile::Dev,
            ),
            Err(ProviderError::InvalidConfiguration(ENV_QA_API_KEY))
        ));
        let long_url = format!("http://127.0.0.1:9/{}", "p".repeat(MAX_BASE_URL_CHARS));
        assert!(matches!(
            QaProviderConfig::with_api_key(
                long_url,
                "key",
                "model",
                "glm",
                Duration::from_secs(5),
                [] as [&str; 0],
                true,
                Profile::Dev,
            ),
            Err(ProviderError::InvalidConfiguration(ENV_QA_BASE_URL))
                | Err(ProviderError::UrlPolicy)
        ));
    }

    #[test]
    fn serialized_request_cap_includes_model() {
        let request = ChatCompletionRequest {
            system: "s".into(),
            user: "u".into(),
        };
        let ok = serialize_chat_request("small-model", &request).unwrap();
        assert!(ok.len() <= MAX_REQUEST_BYTES);
        let huge_user = "x".repeat(MAX_REQUEST_BYTES);
        let oversized = ChatCompletionRequest {
            system: "s".into(),
            user: huge_user,
        };
        assert_eq!(
            serialize_chat_request("model-also-counts", &oversized).unwrap_err(),
            ProviderError::Truncated
        );
    }

    #[test]
    fn cloud_requires_https_allowlist() {
        let err = QaProviderConfig::with_api_key(
            "http://api.example.com/v1",
            "key",
            "model-x",
            "glm",
            Duration::from_secs(5),
            ["api.example.com"],
            false,
            Profile::Prod,
        )
        .unwrap_err();
        assert_eq!(err, ProviderError::UrlPolicy);
    }

    #[test]
    fn allowed_hosts_count_and_length_are_bounded() {
        let too_many: Vec<String> = (0..=MAX_ALLOWED_HOSTS)
            .map(|i| format!("h{i}.example.com"))
            .collect();
        assert!(matches!(
            QaProviderConfig::with_api_key(
                "https://203.0.113.10/v1",
                "key",
                "model",
                "glm",
                Duration::from_secs(5),
                too_many,
                false,
                Profile::Prod,
            ),
            Err(ProviderError::InvalidConfiguration(ENV_QA_ALLOWED_HOSTS))
        ));
        let long_host = format!("{}.example.com", "a".repeat(MAX_ALLOWED_HOST_CHARS));
        assert!(matches!(
            QaProviderConfig::with_api_key(
                "https://203.0.113.10/v1",
                "key",
                "model",
                "glm",
                Duration::from_secs(5),
                [long_host],
                false,
                Profile::Prod,
            ),
            Err(ProviderError::InvalidConfiguration(ENV_QA_ALLOWED_HOSTS))
        ));
    }

    #[test]
    fn parses_structured_payload_and_rejects_oversize() {
        let payload = ProviderGroundedPayload {
            claims: vec![StructuredClaim {
                text: "ok".into(),
                cite_ids: vec!["CITE-0001".into()],
                kind: None,
                value: None,
                unit: None,
            }],
            refusal: false,
        };
        let bytes = serde_json::to_vec(&payload).unwrap();
        assert_eq!(parse_grounded_payload(&bytes).unwrap(), payload);
        let huge = vec![b'x'; MAX_RESPONSE_BYTES + 1];
        assert_eq!(
            parse_grounded_payload(&huge).unwrap_err(),
            ProviderError::Truncated
        );
    }
}
