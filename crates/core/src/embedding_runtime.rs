//! Canonical embedding runtime-path helpers (ADR 0006).
//!
//! Always available — **not** gated behind the `llm` feature — so
//! `fileconv-knowledge` and other non-HTTP consumers share one inference
//! implementation with `fileconv-core::llm` without a feature or dependency cycle.
//!
//! Index signatures include `runtime_path`. Changing inference semantics for a
//! `(base_url, model)` pair that lacked an explicit preset path creates a new
//! generation and triggers desktop reindex.

/// Canonical embedding runtime paths for index signature (ADR 0006).
pub const EMBEDDING_RUNTIME_LOCAL_HASH: &str = "local-hash";
pub const EMBEDDING_RUNTIME_LOCAL_NEURAL: &str = "local-neural";
pub const EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM: &str = "glm-cloud-interim";
pub const EMBEDDING_RUNTIME_VLLM_LOCAL: &str = "vllm-local";
pub const EMBEDDING_RUNTIME_PROVIDER_CLOUD: &str = "provider-cloud";

/// Allowed `runtime_path` values for [`crate::llm::EmbeddingConfig`] / knowledge plans.
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

/// Extract a lowercase host hint used only for runtime-path inference.
///
/// Rules (deterministic):
/// - `None`, blank, or unsupported non-http(s) schemes → empty host
/// - Accepts `http://` / `https://` (case-insensitive) and **scheme-less** URLs
/// - Strips userinfo (`user:pass@`), path, query, fragment, and port
/// - IPv6 literals use bracket form (`[::1]:8000`); brackets are not part of the hint
pub fn embedding_host_hint(base_url: Option<&str>) -> String {
    let Some(value) = base_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };

    let without_scheme = if let Some(rest) = strip_http_scheme(value) {
        rest
    } else if value.contains("://") {
        // ftp://, file://, etc. — not used for embedding endpoints.
        return String::new();
    } else {
        value
    };

    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        return String::new();
    }

    let host_port = match authority.rsplit_once('@') {
        Some((_, host_port)) => host_port,
        None => authority,
    };
    if host_port.is_empty() {
        return String::new();
    }

    let host = if let Some(rest) = host_port.strip_prefix('[') {
        match rest.split_once(']') {
            Some((ipv6, _)) if !ipv6.is_empty() => ipv6,
            _ => return String::new(),
        }
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };

    host.to_ascii_lowercase()
}

fn strip_http_scheme(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    if bytes.len() >= 8 && bytes[..8].eq_ignore_ascii_case(b"https://") {
        Some(&value[8..])
    } else if bytes.len() >= 7 && bytes[..7].eq_ignore_ascii_case(b"http://") {
        Some(&value[7..])
    } else {
        None
    }
}

/// Fallback when no preset supplies `runtime_path` (custom endpoints).
///
/// Prefer explicit preset / config `runtime_path` (desktop vLLM / GLM). Real vLLM
/// preset hosts (`127.0.0.1`) and models (`BAAI/bge-m3`) do **not** contain
/// `"vllm"` — inference alone would mis-label them as [`EMBEDDING_RUNTIME_PROVIDER_CLOUD`].
pub fn infer_embedding_runtime_path(base_url: Option<&str>, model: &str) -> &'static str {
    let host = embedding_host_hint(base_url);
    let model = model.to_ascii_lowercase();
    let blob = format!("{host} {model}");
    if host.contains("bigmodel")
        || host.contains("z.ai")
        || host.contains("zhipu")
        || model.starts_with("embedding-2")
        || model.starts_with("embedding-3")
        || blob.contains("glm")
    {
        return EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM;
    }
    if host.contains("vllm") || model.contains("vllm") {
        return EMBEDDING_RUNTIME_VLLM_LOCAL;
    }
    EMBEDDING_RUNTIME_PROVIDER_CLOUD
}

/// Shared behavior table for core ↔ knowledge parity tests (ADR 0006).
///
/// Each row is `(base_url, model, expected_runtime_path)`.
pub const INFER_EMBEDDING_RUNTIME_PATH_CASES: &[(Option<&str>, &str, &str)] = &[
    // Provider default / empty
    (
        None,
        "text-embedding-3-small",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some(""),
        "text-embedding-3-small",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("   "),
        "text-embedding-3-small",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    // Desktop vLLM preset values must NOT infer vllm-local
    (
        Some("http://127.0.0.1:8000"),
        "BAAI/bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("http://localhost:8000/v1"),
        "BAAI/bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("http://192.168.1.10:8000/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    // GLM / bigmodel / zhipu / z.ai
    (
        Some("https://open.bigmodel.cn/api/paas/v4"),
        "embedding-3",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
    (
        Some("HTTPS://Open.BigModel.CN/api/paas/v4"),
        "other-model",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
    (
        Some("open.bigmodel.cn/api/paas/v4"),
        "custom",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
    (
        Some("https://api.z.ai/v1"),
        "embed",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
    (
        Some("https://open.bigmodel.cn/api/paas/v4"),
        "Embedding-3",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
    (
        Some("https://api.zhipuai.cn/v1"),
        "text",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
    (None, "embedding-2", EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM),
    (None, "embedding-3", EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM),
    (None, "glm-embedding", EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM),
    (
        Some("https://glm.example.com/v1"),
        "text",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
    // vLLM cues
    (
        Some("http://vllm.internal:8000/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_VLLM_LOCAL,
    ),
    (
        Some("vllm.internal:8000/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_VLLM_LOCAL,
    ),
    (
        Some("HTTP://VLLM.INTERNAL:8000/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_VLLM_LOCAL,
    ),
    (None, "vllm-served-model", EMBEDDING_RUNTIME_VLLM_LOCAL),
    // Custom / public provider hosts
    (
        Some("https://api.openai.com/v1"),
        "text-embedding-3-small",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("https://generativelanguage.googleapis.com/v1beta"),
        "text-embedding-004",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("http://custom.embeddings.local:8080/v1"),
        "my-model",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    // Userinfo
    (
        Some("https://user:secret@open.bigmodel.cn/api/paas/v4"),
        "embedding-3",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
    (
        Some("https://user:secret@vllm.internal:8000/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_VLLM_LOCAL,
    ),
    (
        Some("https://user:secret@127.0.0.1:8000/v1"),
        "BAAI/bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    // IPv6 (no vllm/glm cue → provider-cloud)
    (
        Some("http://[::1]:8000/v1"),
        "BAAI/bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("http://[2001:db8::1]:8000/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("http://[vllm::1]:8000/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_VLLM_LOCAL,
    ),
    // Malformed / unsupported
    (
        Some("http://"),
        "text-embedding-3-small",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("://missing-scheme-host"),
        "bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("ftp://vllm.internal/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("not a url at all"),
        "text-embedding-3-small",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    (
        Some("http://[::1"),
        "bge-m3",
        EMBEDDING_RUNTIME_PROVIDER_CLOUD,
    ),
    // GLM wins over vLLM when both cues present
    (
        Some("http://vllm.bigmodel.cn/v1"),
        "bge-m3",
        EMBEDDING_RUNTIME_GLM_CLOUD_INTERIM,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behavior_table_matches_infer_embedding_runtime_path() {
        for (base_url, model, expected) in INFER_EMBEDDING_RUNTIME_PATH_CASES {
            assert_eq!(
                infer_embedding_runtime_path(*base_url, model),
                *expected,
                "base_url={base_url:?} model={model}"
            );
        }
    }

    #[test]
    fn host_hint_handles_scheme_case_userinfo_ipv6_and_scheme_less() {
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
        assert_eq!(embedding_host_hint(Some("http://[::1]:8000/v1")), "::1");
        assert_eq!(
            embedding_host_hint(Some("http://[2001:db8::vllm]:8000/v1")),
            "2001:db8::vllm"
        );
        assert_eq!(embedding_host_hint(Some("ftp://vllm.internal")), "");
        assert_eq!(embedding_host_hint(Some("http://")), "");
        assert_eq!(embedding_host_hint(Some("")), "");
        assert_eq!(embedding_host_hint(None), "");
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

    #[test]
    fn desktop_vllm_preset_inference_stays_provider_cloud() {
        assert_eq!(
            infer_embedding_runtime_path(Some("http://127.0.0.1:8000"), "BAAI/bge-m3"),
            EMBEDDING_RUNTIME_PROVIDER_CLOUD
        );
    }
}
