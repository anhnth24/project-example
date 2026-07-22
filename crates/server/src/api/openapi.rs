//! OpenAPI contract authority + structural drift checks for wired routes (P1B-R06).

use std::collections::BTreeSet;

/// Static OpenAPI document checked into `crates/server/openapi/openapi.yaml`.
pub const OPENAPI_YAML: &str = include_str!("../../openapi/openapi.yaml");

/// Primary HTTP operations currently wired (method lowercase, absolute path).
///
/// Axum `get(...)` also accepts HEAD; inventory/OpenAPI/CORS derive implicit HEAD
/// for every GET. Keep in sync with route modules.
pub const WIRED_OPERATIONS: &[(&str, &str)] = &[
    ("get", "/live"),
    ("get", "/ready"),
    ("get", "/startup"),
    ("get", "/api/v1/health/live"),
    ("get", "/api/v1/health/ready"),
    ("get", "/api/v1/health/startup"),
    ("post", "/api/v1/auth/login"),
    ("post", "/api/v1/auth/refresh"),
    ("post", "/api/v1/auth/logout"),
    ("get", "/api/v1/auth/me"),
    ("get", "/api/v1/collections"),
    ("post", "/api/v1/collections"),
    ("get", "/api/v1/collections/{collectionId}"),
    ("patch", "/api/v1/collections/{collectionId}"),
    ("get", "/api/v1/documents"),
    ("get", "/api/v1/documents/{documentId}"),
    ("delete", "/api/v1/documents/{documentId}"),
    ("post", "/api/v1/documents/{documentId}/reindex"),
    ("get", "/api/v1/documents/{documentId}/versions"),
    ("get", "/api/v1/documents/{documentId}/versions/{versionId}"),
    (
        "post",
        "/api/v1/documents/{documentId}/versions/{versionId}/publish",
    ),
    (
        "get",
        "/api/v1/documents/{documentId}/versions/{leftVersionId}/diff/{rightVersionId}",
    ),
    (
        "get",
        "/api/v1/documents/{documentId}/versions/{versionId}/preview",
    ),
    (
        "post",
        "/api/v1/documents/{documentId}/versions/{versionId}/download-capabilities",
    ),
    ("post", "/api/v1/download-capabilities/redeem"),
    ("get", "/api/v1/conflicts"),
    ("get", "/api/v1/conflicts/{conflictId}"),
    ("post", "/api/v1/conflicts/{conflictId}/triage"),
    ("get", "/api/v1/conflicts/{conflictId}/evidence"),
    ("post", "/api/v1/citations/resolve"),
    ("get", "/api/v1/jobs"),
    ("get", "/api/v1/jobs/{jobId}"),
    ("post", "/api/v1/uploads"),
    ("post", "/api/v1/search"),
    ("post", "/api/v1/ask"),
    ("post", "/api/v1/ask/stream"),
    ("get", "/api/v1/events/{requestId}"),
];

/// Backward-compatible alias used by older drift helpers.
pub const WIRED_API_V1_OPERATIONS: &[(&str, &str)] = WIRED_OPERATIONS;

/// Forbidden substrings that must never appear in the public contract.
const FORBIDDEN_CONTRACT_MARKERS: &[&str] = &[
    "MARKHAND_AUTH_SIGNING_KEY",
    "minio_secret",
    "aws_secret",
    "quarantine/",
    "trusted/",
    "accessKey",
    "secretKey",
    "objectKey",
    "bucketKey",
];

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "trace",
];

/// True when `method` + `path` match a wired operation (template segments allowed).
/// HEAD is recognized for every wired GET (Axum implicit HEAD).
pub fn is_wired_operation(method: &str, path: &str) -> bool {
    let method = method.trim().to_ascii_lowercase();
    wired_operation_set().iter().any(|(wired_method, pattern)| {
        wired_method == &method && path_matches_template(pattern, path)
    })
}

fn path_matches_template(pattern: &str, path: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();
    if pattern_parts.len() != path_parts.len() {
        return false;
    }
    pattern_parts
        .iter()
        .zip(path_parts.iter())
        .all(|(pat, act)| (pat.starts_with('{') && pat.ends_with('}')) || *pat == *act)
}

/// Parse OpenAPI `paths` into `(method, path)` pairs (indentation-based, OpenAPI 3.x).
pub fn parse_openapi_operations(yaml: &str) -> BTreeSet<(String, String)> {
    let mut ops = BTreeSet::new();
    let mut in_paths = false;
    let mut current_path: Option<String> = None;
    for line in yaml.lines() {
        if line.starts_with("paths:") {
            in_paths = true;
            current_path = None;
            continue;
        }
        if !in_paths {
            continue;
        }
        // Leave paths section at next top-level key.
        if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('#') {
            break;
        }
        if let Some(rest) = line.strip_prefix("  /") {
            if let Some(path) = rest.strip_suffix(':') {
                current_path = Some(format!("/{path}"));
                continue;
            }
        }
        let Some(path) = current_path.as_ref() else {
            continue;
        };
        for method in HTTP_METHODS {
            if line == format!("    {method}:") {
                ops.insert(((*method).to_string(), path.clone()));
            }
        }
    }
    ops
}

pub fn wired_operation_set() -> BTreeSet<(String, String)> {
    let mut set = BTreeSet::new();
    for (method, path) in WIRED_OPERATIONS {
        set.insert(((*method).to_string(), (*path).to_string()));
        if *method == "get" {
            set.insert(("head".to_string(), (*path).to_string()));
        }
    }
    set
}

/// Wired operations missing from the OpenAPI document.
pub fn missing_operations(yaml: &str) -> Vec<String> {
    let documented = parse_openapi_operations(yaml);
    let wired = wired_operation_set();
    wired
        .difference(&documented)
        .map(|(method, path)| format!("{} {path}", method.to_uppercase()))
        .collect()
}

/// OpenAPI operations that are not wired in the server.
pub fn extra_operations(yaml: &str) -> Vec<String> {
    let documented = parse_openapi_operations(yaml);
    let wired = wired_operation_set();
    documented
        .difference(&wired)
        .map(|(method, path)| format!("{} {path}", method.to_uppercase()))
        .collect()
}

pub fn forbidden_markers(yaml: &str) -> Vec<&'static str> {
    FORBIDDEN_CONTRACT_MARKERS
        .iter()
        .copied()
        .filter(|marker| yaml.contains(marker))
        .collect()
}

/// Canonical SSE application event names (transport heartbeats are comments only).
pub const CANONICAL_SSE_EVENTS: &[&str] = &[
    crate::api::sse::EVENT_METADATA,
    crate::api::sse::EVENT_TOKEN,
    crate::api::sse::EVENT_CLOSE,
    crate::api::sse::EVENT_ERROR,
];

/// Parse repeated SSE frames from the checked-in fixture (`id`/`event`/`data` blocks).
pub fn parse_sse_fixture_envelopes(
    raw: &str,
) -> Result<Vec<crate::api::types::SseEnvelope>, String> {
    let mut envelopes = Vec::new();
    let mut data_lines: Vec<&str> = Vec::new();
    for line in raw.lines() {
        if line.is_empty() {
            if data_lines.is_empty() {
                continue;
            }
            let joined = data_lines.join("\n");
            let envelope: crate::api::types::SseEnvelope = serde_json::from_str(&joined)
                .map_err(|error| format!("sse fixture data JSON: {error}"))?;
            envelopes.push(envelope);
            data_lines.clear();
            continue;
        }
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start());
        }
    }
    if !data_lines.is_empty() {
        let joined = data_lines.join("\n");
        let envelope: crate::api::types::SseEnvelope = serde_json::from_str(&joined)
            .map_err(|error| format!("sse fixture data JSON: {error}"))?;
        envelopes.push(envelope);
    }
    if envelopes.is_empty() {
        return Err("sse fixture contained no frames".into());
    }
    Ok(envelopes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_snapshot_matches_embedded_bytes() {
        assert!(OPENAPI_YAML.contains("openapi: 3.1.0"));
        assert!(OPENAPI_YAML.contains("title: Markhand Web API"));
        assert!(OPENAPI_YAML.len() > 2_000);
    }

    #[test]
    fn structural_route_method_inventory_is_bidirectional() {
        let missing = missing_operations(OPENAPI_YAML);
        let extra = extra_operations(OPENAPI_YAML);
        assert!(
            missing.is_empty(),
            "OpenAPI drift — missing operations: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "OpenAPI drift — extra operations: {extra:?}"
        );
        // Implicit HEAD for every GET is part of the inventory.
        assert!(wired_operation_set().contains(&("head".into(), "/api/v1/auth/me".into())));
    }

    #[test]
    fn contract_does_not_expose_secrets_or_object_keys() {
        let bad = forbidden_markers(OPENAPI_YAML);
        assert!(bad.is_empty(), "forbidden markers present: {bad:?}");
    }

    #[test]
    fn documents_security_errors_rate_limit_headers_and_sse() {
        assert!(OPENAPI_YAML.contains("bearerAuth"));
        assert!(OPENAPI_YAML.contains("RateLimited"));
        assert!(OPENAPI_YAML.contains("text/event-stream"));
        assert!(OPENAPI_YAML.contains("SseEnvelope"));
        assert!(OPENAPI_YAML.contains("Retry-After"));
        assert!(OPENAPI_YAML.contains("X-RateLimit-Limit"));
        assert!(OPENAPI_YAML.contains("application/octet-stream"));
    }

    #[test]
    fn wire_schema_markers_match_handlers() {
        assert!(OPENAPI_YAML.contains("purpose"));
        assert!(OPENAPI_YAML.contains("citations"));
        assert!(OPENAPI_YAML.contains("chunkId"));
        assert!(OPENAPI_YAML.contains("SearchHit"));
        assert!(OPENAPI_YAML.contains("AskCitation"));
        assert!(OPENAPI_YAML.contains("VersionMode"));
        // Token response tokens are not writeOnly.
        assert!(OPENAPI_YAML.contains("TokenResponse"));
    }

    #[test]
    fn sse_fixture_is_repeated_event_stream_frames() {
        let raw = include_str!("../../openapi/fixtures/sse.event-stream");
        let envelopes = parse_sse_fixture_envelopes(raw).unwrap();
        assert!(envelopes.len() >= 2);
        for envelope in &envelopes {
            assert!(
                CANONICAL_SSE_EVENTS.contains(&envelope.event.as_str()),
                "fixture event {:?} is not a canonical SSE event",
                envelope.event
            );
        }
        assert_eq!(envelopes[0].event, crate::api::sse::EVENT_METADATA);
        assert_eq!(envelopes[0].data["mode"], "offline_extractive");
        assert_eq!(envelopes[0].data["answerMode"], "offline_extractive");
        assert!(
            matches!(
                envelopes[0].data["mode"].as_str(),
                Some("offline_extractive" | "fallback_extractive" | "provider_llm")
            ),
            "fixture metadata mode must use AnswerMode::as_str()"
        );
    }

    #[test]
    fn path_template_matching_for_cors() {
        assert!(is_wired_operation(
            "GET",
            "/api/v1/collections/550e8400-e29b-41d4-a716-446655440000"
        ));
        assert!(is_wired_operation(
            "HEAD",
            "/api/v1/collections/550e8400-e29b-41d4-a716-446655440000"
        ));
        assert!(!is_wired_operation(
            "POST",
            "/api/v1/collections/nope/extra"
        ));
        assert!(is_wired_operation("GET", "/live"));
        assert!(is_wired_operation("HEAD", "/live"));
    }
}
