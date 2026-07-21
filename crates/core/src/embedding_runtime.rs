//! Canonical embedding runtime-path helpers (ADR 0006).
//!
//! Always available — **not** gated behind the `llm` feature — so
//! `fileconv-knowledge` and other non-HTTP consumers share one inference
//! implementation with `fileconv-core::llm` without a feature or dependency cycle.
//!
//! Index signatures include `runtime_path`. Changing inference semantics for a
//! `(base_url, model)` pair that lacked an explicit preset path creates a new
//! generation and triggers desktop reindex.

use url::Url;

/// Canonical embedding runtime paths for index signature (ADR 0006).
pub const EMBEDDING_RUNTIME_LOCAL_HASH: &str = "local-hash";
pub const EMBEDDING_RUNTIME_LOCAL_NEURAL: &str = "local-neural";
pub const EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM: &str = "glm-cloud-interim";
pub const EMBEDDING_RUNTIME_VLLM_LOCAL: &str = "vllm-local";
pub const EMBEDDING_RUNTIME_PROVIDER_CLOUD: &str = "provider-cloud";

/// Allowed `runtime_path` values for embedding configs and knowledge plans.
pub const ALLOWED_EMBEDDING_RUNTIME_PATHS: &[&str] = &[
    EMBEDDING_RUNTIME_LOCAL_HASH,
    EMBEDDING_RUNTIME_LOCAL_NEURAL,
    EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    EMBEDDING_RUNTIME_VLLM_LOCAL,
    EMBEDDING_RUNTIME_PROVIDER_CLOUD,
];

pub fn is_allowed_embedding_runtime_path(path: &str) -> bool {
    ALLOWED_EMBEDDING_RUNTIME_PATHS.contains(&path)
}

/// Official GLM / Zhipu cloud embedding hosts (DNS suffix match).
const OFFICIAL_GLM_DOMAINS: &[&str] = &["bigmodel.cn", "z.ai", "zhipuai.cn"];

/// Public provider hosts that resolve to [`EMBEDDING_RUNTIME_PROVIDER_CLOUD`]
/// before model-name cues are considered.
const KNOWN_PROVIDER_DOMAINS: &[&str] = &["openai.com", "googleapis.com"];

/// Extract a lowercase host hint used only for runtime-path inference.
///
/// Rules (deterministic):
/// - `None` / blank → empty host
/// - HTTP(S) parsed with [`Url`] (case-insensitive scheme)
/// - Scheme-less values get an `https://` prefix **only** when syntactically
///   plausible; otherwise empty host
/// - Non-http(s) / malformed → empty host (silent)
/// - Userinfo / path / query / fragment stripped by the URL parser
pub fn embedding_host_hint(base_url: Option<&str>) -> String {
    parse_embedding_endpoint(base_url)
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
        .unwrap_or_default()
}

fn parse_embedding_endpoint(base_url: Option<&str>) -> Option<Url> {
    let value = base_url.map(str::trim).filter(|value| !value.is_empty())?;

    if let Ok(url) = Url::parse(value) {
        if matches!(url.scheme(), "http" | "https") {
            return url.host_str().is_some().then_some(url);
        }
        // Absolute non-http(s), or WHATWG treating `host:port` as a scheme.
        if value.contains("://") {
            return None;
        }
    } else if value.contains("://") {
        // Malformed absolute URL (e.g. `http://[vllm::1]`).
        return None;
    }

    if !scheme_less_plausible(value) {
        return None;
    }
    let prefixed = format!("https://{value}");
    let url = Url::parse(&prefixed).ok()?;
    if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() {
        Some(url)
    } else {
        None
    }
}

fn scheme_less_plausible(value: &str) -> bool {
    if value.contains("://") || value.contains('\\') {
        return false;
    }
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let authority = value.split(['/', '?', '#']).next().unwrap_or("");
    let Some(first) = authority.chars().next() else {
        return false;
    };
    first.is_ascii_alphanumeric() || first == '['
}

fn host_for_dns_match(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
}

/// DNS-label-boundary domain match: `z.ai` matches `api.z.ai`, not `modelz.ai`.
fn host_matches_domain(host: &str, domain: &str) -> bool {
    let host = host_for_dns_match(host);
    if host == domain {
        return true;
    }
    let Some(prefix_len) = host.len().checked_sub(domain.len()) else {
        return false;
    };
    prefix_len > 0 && host.as_bytes().get(prefix_len - 1) == Some(&b'.') && host.ends_with(domain)
}

fn host_has_dns_label(host: &str, label: &str) -> bool {
    host_for_dns_match(host)
        .split('.')
        .any(|part| part == label)
}

fn is_official_glm_host(host: &str) -> bool {
    !host.is_empty()
        && OFFICIAL_GLM_DOMAINS
            .iter()
            .any(|domain| host_matches_domain(host, domain))
}

fn is_vllm_host(host: &str) -> bool {
    host_has_dns_label(host, "vllm")
}

fn is_loopback_host(host: &str) -> bool {
    let host = host_for_dns_match(host);
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0:0:0:0:0:0:0:1")
}

fn is_known_provider_host(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }
    if is_loopback_host(host) {
        return true;
    }
    KNOWN_PROVIDER_DOMAINS
        .iter()
        .any(|domain| host_matches_domain(host, domain))
}

fn model_has_anchored_token(model: &str, token: &str) -> bool {
    model
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| part == token)
}

fn anchored_glm_model_cue(model: &str) -> bool {
    model.starts_with("embedding-2")
        || model.starts_with("embedding-3")
        || model_has_anchored_token(model, "glm")
}

fn anchored_vllm_model_cue(model: &str) -> bool {
    model_has_anchored_token(model, "vllm")
}

/// Fallback when no preset supplies `runtime_path` (custom endpoints).
///
/// Cue order (ADR 0006 / CORE-T13):
/// 1. official GLM host (`*.bigmodel.cn`, `*.z.ai`, `*.zhipuai.cn`)
/// 2. vLLM host (DNS label `vllm`) — beats GLM-named models
/// 3. known provider host / loopback → [`EMBEDDING_RUNTIME_PROVIDER_CLOUD`]
/// 4. carefully anchored model cues (`embedding-2|3`, token `glm` / `vllm`)
/// 5. default [`EMBEDDING_RUNTIME_PROVIDER_CLOUD`]
///
/// Prefer explicit preset `runtime_path` (desktop vLLM / GLM). Real vLLM preset
/// hosts (`127.0.0.1`) and models (`BAAI/bge-m3`) do **not** contain a `vllm`
/// DNS label — inference alone yields [`EMBEDDING_RUNTIME_PROVIDER_CLOUD`].
pub fn infer_embedding_runtime_path(base_url: Option<&str>, model: &str) -> &'static str {
    let host = embedding_host_hint(base_url);
    let model = model.trim().to_ascii_lowercase();

    if is_official_glm_host(&host) {
        return EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM;
    }
    if is_vllm_host(&host) {
        return EMBEDDING_RUNTIME_VLLM_LOCAL;
    }
    if is_known_provider_host(&host) {
        return EMBEDDING_RUNTIME_PROVIDER_CLOUD;
    }
    if anchored_glm_model_cue(&model) {
        return EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM;
    }
    if anchored_vllm_model_cue(&model) {
        return EMBEDDING_RUNTIME_VLLM_LOCAL;
    }
    EMBEDDING_RUNTIME_PROVIDER_CLOUD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_inference_cases() {
        let cases: &[(Option<&str>, &str, &str)] = &[
            // Empty / default
            (None, "text-embedding-3-small", "provider-cloud"),
            (Some(""), "text-embedding-3-small", "provider-cloud"),
            (Some("   "), "text-embedding-3-small", "provider-cloud"),
            // Desktop vLLM preset values must NOT infer vllm-local
            (
                Some("http://127.0.0.1:8000"),
                "BAAI/bge-m3",
                "provider-cloud",
            ),
            (
                Some("http://localhost:8000/v1"),
                "BAAI/bge-m3",
                "provider-cloud",
            ),
            (
                Some("http://192.168.1.10:8000/v1"),
                "bge-m3",
                "provider-cloud",
            ),
            // Official GLM hosts (schemed, casing, scheme-less)
            (
                Some("https://open.bigmodel.cn/api/paas/v4"),
                "embedding-3",
                "glm-cloud-interim",
            ),
            (
                Some("HTTPS://Open.BigModel.CN/api/paas/v4"),
                "other-model",
                "glm-cloud-interim",
            ),
            (
                Some("open.bigmodel.cn/api/paas/v4"),
                "custom",
                "glm-cloud-interim",
            ),
            (Some("https://api.z.ai/v1"), "embed", "glm-cloud-interim"),
            (
                Some("https://api.zhipuai.cn/v1"),
                "text",
                "glm-cloud-interim",
            ),
            // DNS label boundary: modelz.ai must not match z.ai
            (Some("https://modelz.ai/v1"), "embed", "provider-cloud"),
            (Some("https://notbigmodel.cn/v1"), "embed", "provider-cloud"),
            // Non-official host with "glm" label → not official host; no model cue
            (Some("https://glm.example.com/v1"), "text", "provider-cloud"),
            // Anchored model cues (no decisive host)
            (None, "embedding-2", "glm-cloud-interim"),
            (None, "embedding-3", "glm-cloud-interim"),
            (None, "glm-embedding", "glm-cloud-interim"),
            (None, "org/glm-embed", "glm-cloud-interim"),
            (None, "myglmmodel", "provider-cloud"),
            (None, "vllm-served-model", "vllm-local"),
            (None, "myvllmserved", "provider-cloud"),
            // vLLM hosts
            (Some("http://vllm.internal:8000/v1"), "bge-m3", "vllm-local"),
            (Some("vllm.internal:8000/v1"), "bge-m3", "vllm-local"),
            (Some("HTTP://VLLM.INTERNAL:8000/v1"), "bge-m3", "vllm-local"),
            // vLLM host beats GLM-named model
            (
                Some("http://vllm.internal:8000/v1"),
                "glm-embedding",
                "vllm-local",
            ),
            (
                Some("http://vllm.internal:8000/v1"),
                "embedding-3",
                "vllm-local",
            ),
            // Official GLM host beats vLLM DNS label (vllm.bigmodel.cn)
            (
                Some("http://vllm.bigmodel.cn/v1"),
                "bge-m3",
                "glm-cloud-interim",
            ),
            // Known provider hosts beat model cues
            (
                Some("https://api.openai.com/v1"),
                "embedding-3",
                "provider-cloud",
            ),
            (
                Some("https://generativelanguage.googleapis.com/v1beta"),
                "glm-embedding",
                "provider-cloud",
            ),
            (
                Some("http://127.0.0.1:8000"),
                "embedding-3",
                "provider-cloud",
            ),
            // Custom host still allows anchored model cues
            (
                Some("http://custom.embeddings.local:8080/v1"),
                "embedding-3",
                "glm-cloud-interim",
            ),
            (
                Some("http://custom.embeddings.local:8080/v1"),
                "my-model",
                "provider-cloud",
            ),
            // Userinfo (legitimate)
            (
                Some("https://user:secret@open.bigmodel.cn/api/paas/v4"),
                "embedding-3",
                "glm-cloud-interim",
            ),
            (
                Some("https://user:secret@vllm.internal:8000/v1"),
                "bge-m3",
                "vllm-local",
            ),
            // Evil backslash-userinfo must not spoof official/vLLM hosts
            (
                Some(r"https://evil.com\@open.bigmodel.cn/v1"),
                "bge-m3",
                "provider-cloud",
            ),
            (
                Some(r"https://evil\bigmodel.cn@127.0.0.1/v1"),
                "bge-m3",
                "provider-cloud",
            ),
            (
                Some(r"https://evil.com\@vllm.internal/v1"),
                "bge-m3",
                "provider-cloud",
            ),
            // Same evil host + anchored model cue (host is evil.com, not known provider)
            (
                Some(r"https://evil.com\@open.bigmodel.cn/v1"),
                "embedding-3",
                "glm-cloud-interim",
            ),
            // IPv6 loopback / valid
            (
                Some("http://[::1]:8000/v1"),
                "BAAI/bge-m3",
                "provider-cloud",
            ),
            (
                Some("http://[2001:db8::1]:8000/v1"),
                "bge-m3",
                "provider-cloud",
            ),
            // Invalid IPv6 with vllm-looking text → silent default (unless model cue)
            (Some("http://[vllm::1]:8000/v1"), "bge-m3", "provider-cloud"),
            (
                Some("http://[vllm::1]:8000/v1"),
                "embedding-3",
                "glm-cloud-interim",
            ),
            // Malformed / non-http → silent default unless anchored model cue
            (Some("http://"), "text-embedding-3-small", "provider-cloud"),
            (Some("://missing-scheme-host"), "bge-m3", "provider-cloud"),
            (Some("ftp://vllm.internal/v1"), "bge-m3", "provider-cloud"),
            (Some("ftp://vllm.internal/v1"), "vllm-served", "vllm-local"),
            (
                Some("not a url at all"),
                "text-embedding-3-small",
                "provider-cloud",
            ),
            (Some("http://[::1"), "bge-m3", "provider-cloud"),
        ];

        for (base_url, model, expected) in cases {
            assert_eq!(
                infer_embedding_runtime_path(*base_url, model),
                *expected,
                "base_url={base_url:?} model={model}"
            );
        }
    }

    #[test]
    fn host_hint_uses_url_parser() {
        assert_eq!(
            embedding_host_hint(Some("HTTPS://Open.BigModel.CN/api")),
            "open.bigmodel.cn"
        );
        assert_eq!(
            embedding_host_hint(Some("open.bigmodel.cn/api")),
            "open.bigmodel.cn"
        );
        assert_eq!(
            embedding_host_hint(Some("https://user:pass@vllm.internal:8000/v1")),
            "vllm.internal"
        );
        assert_eq!(embedding_host_hint(Some("http://[::1]:8000/v1")), "[::1]");
        assert_eq!(embedding_host_hint(Some("ftp://vllm.internal")), "");
        assert_eq!(embedding_host_hint(Some("http://[vllm::1]:8000/v1")), "");
        assert_eq!(
            embedding_host_hint(Some(r"https://evil.com\@open.bigmodel.cn/v1")),
            "evil.com"
        );
        assert_eq!(embedding_host_hint(Some("http://")), "");
        assert_eq!(embedding_host_hint(None), "");
    }

    #[test]
    fn dns_label_boundaries_reject_substring_hosts() {
        assert!(host_matches_domain("api.z.ai", "z.ai"));
        assert!(!host_matches_domain("modelz.ai", "z.ai"));
        assert!(host_matches_domain("open.bigmodel.cn", "bigmodel.cn"));
        assert!(!host_matches_domain("notbigmodel.cn", "bigmodel.cn"));
        assert!(host_has_dns_label("vllm.internal", "vllm"));
        assert!(!host_has_dns_label("myvllm.internal", "vllm"));
    }

    #[test]
    fn allowed_runtime_paths_cover_adr_values() {
        for path in [
            EMBEDDING_RUNTIME_LOCAL_HASH,
            EMBEDDING_RUNTIME_LOCAL_NEURAL,
            EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
            EMBEDDING_RUNTIME_VLLM_LOCAL,
            EMBEDDING_RUNTIME_PROVIDER_CLOUD,
        ] {
            assert!(is_allowed_embedding_runtime_path(path));
        }
        assert!(!is_allowed_embedding_runtime_path("local_hash_v1"));
    }
}
