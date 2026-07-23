//! Embedded OpenAPI helpers and route/schema parity inventory (P1B-R06).
//!
//! Two-way parity: every inventory route must appear in OpenAPI, and every
//! OpenAPI `/` path (except documented exclusions) must appear in inventory.

/// Canonical (method, path, required status codes) for every shipped `/api/v1` route.
/// Paths are OpenAPI-relative (no `/api/v1` prefix). Parity is structural — not substring.
pub const ROUTE_INVENTORY: &[(&str, &str, &[&str])] = &[
    ("get", "/health/live", &["200"]),
    ("get", "/health/ready", &["200", "503"]),
    ("get", "/health/start", &["200", "503"]),
    ("post", "/auth/login", &["200", "401", "429"]),
    ("post", "/auth/refresh", &["200", "401", "429"]),
    ("post", "/auth/logout", &["204", "429"]),
    ("get", "/auth/me", &["200", "401", "429"]),
    ("post", "/uploads", &["201", "400", "403", "413", "429"]),
    ("get", "/collections", &["200", "429"]),
    ("post", "/collections", &["201", "403", "429"]),
    ("get", "/collections/{collectionId}", &["200", "404", "429"]),
    (
        "patch",
        "/collections/{collectionId}",
        &["200", "404", "429"],
    ),
    (
        "delete",
        "/collections/{collectionId}",
        &["204", "404", "429"],
    ),
    (
        "get",
        "/collections/{collectionId}/documents",
        &["200", "404", "429"],
    ),
    (
        "post",
        "/collections/{collectionId}/documents/{documentId}/approve-intake",
        &["200", "403", "404", "429"],
    ),
    ("get", "/documents/{documentId}", &["200", "404", "429"]),
    ("delete", "/documents/{documentId}", &["204", "404", "429"]),
    (
        "get",
        "/documents/{documentId}/preview",
        &["200", "403", "404", "429"],
    ),
    ("get", "/documents/{documentId}/versions", &["200", "429"]),
    (
        "get",
        "/documents/{documentId}/versions/{versionId}",
        &["200", "403", "404", "429"],
    ),
    (
        "get",
        "/documents/{documentId}/versions/{versionId}/diff",
        &["200", "403", "404", "429"],
    ),
    (
        "post",
        "/documents/{documentId}/versions/{versionId}/publish",
        &["204", "429"],
    ),
    (
        "post",
        "/documents/{documentId}/versions/{versionId}/download-capability",
        &["200", "429"],
    ),
    ("get", "/downloads/{capability}", &["200", "429"]),
    ("post", "/documents/{documentId}/reindex", &["200", "429"]),
    ("post", "/citations/resolve", &["200", "429"]),
    ("get", "/conflicts", &["200", "429"]),
    ("get", "/conflicts/{conflictId}", &["200", "429"]),
    ("get", "/conflicts/{conflictId}/evidence", &["200", "429"]),
    ("post", "/conflicts/{conflictId}/triage", &["200", "429"]),
    ("get", "/jobs/{jobId}", &["200", "404", "429"]),
    (
        "get",
        "/jobs/{jobId}/events",
        &["200", "400", "401", "404", "429"],
    ),
    ("post", "/search", &["200", "400", "401", "403", "429"]),
    ("post", "/ask", &["200", "400", "401", "403", "429"]),
    (
        "post",
        "/ask/stream",
        &["200", "400", "401", "403", "404", "429"],
    ),
    ("get", "/openapi.yaml", &["200", "429"]),
];

const HEALTH_PATHS: &[&str] = &["/health/live", "/health/ready", "/health/start"];

pub fn embedded_openapi_yaml() -> &'static str {
    include_str!("../../openapi/openapi.yaml")
}

pub fn openapi_path_count() -> usize {
    embedded_openapi_yaml()
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with('/') && trimmed.ends_with(':')
        })
        .count()
}

/// Inventory → OpenAPI structural gaps.
pub fn openapi_inventory_gaps(yaml: &str) -> Vec<String> {
    let mut gaps = Vec::new();
    for &(method, path, statuses) in ROUTE_INVENTORY {
        let Some(path_block) = extract_path_block(yaml, path) else {
            gaps.push(format!("missing path {path}"));
            continue;
        };
        if !method_present(path_block, method) {
            gaps.push(format!("missing method {method} on {path}"));
            continue;
        }
        let Some(op_block) = extract_method_block(path_block, method) else {
            gaps.push(format!("unreadable method {method} on {path}"));
            continue;
        };
        for status in statuses {
            let needle = format!("\"{status}\":");
            let needle_alt = format!("'{status}':");
            if !op_block.contains(&needle) && !op_block.contains(&needle_alt) {
                gaps.push(format!("missing status {status} on {method} {path}"));
            }
        }
        // Non-health runtime paths (including OpenAPI document) must document 429.
        if !HEALTH_PATHS.contains(&path) && !statuses.contains(&"429") {
            gaps.push(format!("inventory missing 429 for {method} {path}"));
        }
    }
    for marker in [
        "bearerAuth:",
        "text/event-stream:",
        "multipart/form-data:",
        "Retry-After:",
        "RateLimited:",
        "SseEnvelope:",
        "ApiError:",
        "streamSessionId",
    ] {
        if !yaml.contains(marker) {
            gaps.push(format!("missing marker {marker}"));
        }
    }
    gaps
}

/// OpenAPI → inventory gaps (orphan OpenAPI paths/methods).
pub fn openapi_yaml_gaps(yaml: &str) -> Vec<String> {
    let mut gaps = Vec::new();
    let inventory: std::collections::BTreeSet<(&str, &str)> = ROUTE_INVENTORY
        .iter()
        .map(|&(method, path, _)| (method, path))
        .collect();
    for path in openapi_paths(yaml) {
        let Some(block) = extract_path_block(yaml, path) else {
            continue;
        };
        for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
            if method_present(block, method) && !inventory.contains(&(method, path)) {
                gaps.push(format!("openapi orphan {method} {path}"));
            }
        }
    }
    gaps
}

/// Two-way router↔OpenAPI parity using the shared inventory as the router contract.
pub fn router_openapi_parity_gaps(yaml: &str) -> Vec<String> {
    let mut gaps = openapi_inventory_gaps(yaml);
    gaps.extend(openapi_yaml_gaps(yaml));
    gaps
}

fn openapi_paths(yaml: &str) -> Vec<&str> {
    yaml.lines()
        .filter_map(|line| {
            let trimmed = line.trim_end();
            if trimmed.starts_with("  /") && trimmed.ends_with(':') {
                Some(&trimmed[2..trimmed.len() - 1])
            } else {
                None
            }
        })
        .collect()
}

fn method_present(path_block: &str, method: &str) -> bool {
    path_block
        .lines()
        .any(|line| line == format!("    {method}:"))
}

fn extract_path_block<'a>(yaml: &'a str, path: &str) -> Option<&'a str> {
    let header = format!("  {path}:\n");
    let start = yaml.find(&header)?;
    let rest = &yaml[start + header.len()..];
    let mut offset = 0usize;
    for line in rest.lines() {
        if (line.starts_with("  /") && line.ends_with(':')) || line == "components:" {
            return Some(&rest[..offset.saturating_sub(1).min(rest.len())]);
        }
        offset += line.len() + 1;
    }
    Some(rest)
}

fn extract_method_block<'a>(path_block: &'a str, method: &str) -> Option<&'a str> {
    let header = format!("    {method}:\n");
    let start = path_block.find(&header)?;
    let rest = &path_block[start..];
    let mut offset = 0usize;
    for (idx, line) in rest.lines().enumerate() {
        if idx == 0 {
            offset += line.len() + 1;
            continue;
        }
        if line.starts_with("    ")
            && !line.starts_with("     ")
            && matches!(
                line.trim_end_matches(':'),
                "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "parameters"
            )
        {
            return Some(&rest[..offset.saturating_sub(1)]);
        }
        offset += line.len() + 1;
    }
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_inventory_is_structurally_complete_two_way() {
        let yaml = embedded_openapi_yaml();
        let gaps = router_openapi_parity_gaps(yaml);
        assert!(
            gaps.is_empty(),
            "OpenAPI/router parity gaps: {}",
            gaps.join("; ")
        );
        assert!(openapi_path_count() >= 20);
        assert!(yaml.contains("sourceContentSha256"));
        assert!(!yaml.contains("contentSha256:"));
    }
}
