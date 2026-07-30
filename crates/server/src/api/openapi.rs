//! Embedded OpenAPI helpers and route/schema parity inventory (P1B-R06).
//!
//! Two-way parity: every inventory route must appear in OpenAPI, and every
//! OpenAPI `/` path (except documented exclusions) must appear in inventory.
//!
//! Schema completeness (separate from route/method/status parity above): every
//! body-taking operation must declare a `requestBody`, and every 2xx response
//! other than 204 must declare response `content`. This is the check that
//! catches a path/method/status being present but payload-less (the gap that
//! route/method/status parity alone cannot see).

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
    ("post", "/collections", &["201", "403", "409", "429"]),
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
        "post",
        "/collections/{collectionId}/assign-project",
        &["200", "403", "404", "429"],
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
    (
        "post",
        "/search",
        &["200", "400", "401", "403", "404", "429"],
    ),
    ("post", "/ask", &["200", "400", "401", "403", "404", "429"]),
    (
        "post",
        "/ask/stream",
        &["200", "400", "401", "403", "404", "429"],
    ),
    ("get", "/openapi.yaml", &["200", "429"]),
    // 1C-01 org lifecycle: create/list/detail/switch (auth-only — see routes/orgs.rs).
    ("get", "/orgs", &["200", "401", "429"]),
    ("post", "/orgs", &["201", "400", "401", "409", "429"]),
    ("get", "/orgs/{orgId}", &["200", "401", "404", "429"]),
    ("post", "/orgs/switch", &["200", "401", "403", "429"]),
    // P2-18 org -> project -> collection -> document grouping. GET has no
    // 403: same as GET /collections, list_projects is unfiltered by
    // permission (`org membership` is the only gate) — see routes::projects.
    ("get", "/projects", &["200", "429"]),
    ("post", "/projects", &["201", "400", "403", "409", "429"]),
    (
        "patch",
        "/projects/{projectId}",
        &["200", "400", "403", "404", "429"],
    ),
    // P2-11 / P2-12 membership + invite + usage admin surface (Wave 2).
    ("get", "/members", &["200", "403", "429"]),
    ("get", "/members/invites", &["200", "403", "429"]),
    ("post", "/members/invites", &["201", "400", "403", "429"]),
    (
        "post",
        "/members/invites/{inviteId}/revoke",
        &["200", "403", "404", "409", "429"],
    ),
    (
        "post",
        "/members/invites/accept",
        &["201", "400", "401", "404", "409", "429"],
    ),
    (
        "patch",
        "/members/{userId}",
        &["200", "400", "403", "404", "409", "429"],
    ),
    (
        "delete",
        "/members/{userId}",
        &["204", "403", "404", "409", "429"],
    ),
    ("get", "/usage", &["200", "403", "429"]),
    // 1C-11 audit-log read endpoint (write path pre-existing; this is the
    // first read surface — see routes/audit.rs).
    ("get", "/audit", &["200", "400", "403", "429"]),
    // P2-17 Document Graph MVP — see routes/graph.rs.
    ("get", "/graph", &["200", "403", "404", "429"]),
    // P2-19 private per-user Q&A chat history — see routes/chat_sessions.rs.
    ("get", "/chat-sessions", &["200", "400", "403", "429"]),
    ("post", "/chat-sessions", &["201", "400", "403", "429"]),
    (
        "get",
        "/chat-sessions/{sessionId}",
        &["200", "403", "404", "429"],
    ),
    (
        "patch",
        "/chat-sessions/{sessionId}",
        &["200", "400", "403", "404", "429"],
    ),
    (
        "delete",
        "/chat-sessions/{sessionId}",
        &["204", "403", "404", "429"],
    ),
    (
        "post",
        "/chat-sessions/{sessionId}/turns",
        &["201", "400", "403", "404", "429"],
    ),
];

const HEALTH_PATHS: &[&str] = &["/health/live", "/health/ready", "/health/start"];

/// Operations whose route handler actually reads a request body (JSON or
/// multipart), used to assert every such operation declares an OpenAPI
/// `requestBody`. Deliberately excludes bodyless POST routes — e.g.
/// `publish` (`documents.rs::publish_version`) and `reindex`
/// (`documents.rs::reindex_document`) take no `Json<...>`/body extractor at
/// all, so they must NOT be listed here or the check would demand a
/// `requestBody` the handler doesn't accept.
pub const BODY_TAKING_OPERATIONS: &[(&str, &str)] = &[
    ("post", "/auth/login"),
    ("post", "/auth/refresh"),
    ("post", "/auth/logout"),
    ("post", "/uploads"),
    ("post", "/collections"),
    ("patch", "/collections/{collectionId}"),
    ("post", "/collections/{collectionId}/assign-project"),
    (
        "post",
        "/collections/{collectionId}/documents/{documentId}/approve-intake",
    ),
    (
        "post",
        "/documents/{documentId}/versions/{versionId}/download-capability",
    ),
    ("post", "/citations/resolve"),
    ("post", "/conflicts/{conflictId}/triage"),
    ("post", "/search"),
    ("post", "/ask"),
    ("post", "/ask/stream"),
    ("post", "/orgs"),
    ("post", "/orgs/switch"),
    ("post", "/projects"),
    ("patch", "/projects/{projectId}"),
    ("post", "/members/invites"),
    ("post", "/members/invites/accept"),
    ("patch", "/members/{userId}"),
    ("post", "/chat-sessions"),
    ("patch", "/chat-sessions/{sessionId}"),
    ("post", "/chat-sessions/{sessionId}/turns"),
];

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

/// Every operation in [`BODY_TAKING_OPERATIONS`] must declare a `requestBody`.
/// Fail-closed: an unreadable path/method is itself reported as a gap rather
/// than silently skipped.
pub fn openapi_request_body_gaps(yaml: &str) -> Vec<String> {
    let mut gaps = Vec::new();
    for &(method, path) in BODY_TAKING_OPERATIONS {
        let Some(path_block) = extract_path_block(yaml, path) else {
            gaps.push(format!("missing path {path} for requestBody check"));
            continue;
        };
        let Some(op_block) = extract_method_block(path_block, method) else {
            gaps.push(format!(
                "unreadable method {method} on {path} for requestBody check"
            ));
            continue;
        };
        if !op_block.contains("requestBody:") {
            gaps.push(format!("missing requestBody on {method} {path}"));
        }
    }
    gaps
}

/// Every 2xx response other than 204 (across [`ROUTE_INVENTORY`]) must declare
/// response `content`. Fail-closed: an unreadable status block is itself
/// reported as a gap rather than silently skipped.
pub fn openapi_response_content_gaps(yaml: &str) -> Vec<String> {
    let mut gaps = Vec::new();
    for &(method, path, statuses) in ROUTE_INVENTORY {
        let Some(path_block) = extract_path_block(yaml, path) else {
            // Already reported by openapi_inventory_gaps.
            continue;
        };
        let Some(op_block) = extract_method_block(path_block, method) else {
            continue;
        };
        for &status in statuses {
            if status == "204" || !status.starts_with('2') {
                continue;
            }
            match extract_status_block(op_block, status) {
                Some(status_block) if status_block.contains("content:") => {}
                Some(_) => gaps.push(format!(
                    "missing response content on {status} for {method} {path}"
                )),
                None => gaps.push(format!(
                    "unreadable status {status} on {method} {path} for content check"
                )),
            }
        }
    }
    gaps
}

/// Combined schema-completeness gaps: requestBody presence + 2xx response content.
pub fn openapi_schema_completeness_gaps(yaml: &str) -> Vec<String> {
    let mut gaps = openapi_request_body_gaps(yaml);
    gaps.extend(openapi_response_content_gaps(yaml));
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
    exact_line_span(path_block, &format!("    {method}:")).is_some()
}

fn extract_path_block<'a>(yaml: &'a str, path: &str) -> Option<&'a str> {
    let header = format!("  {path}:");
    let (_, after_header) = exact_line_span(yaml, &header)?;
    let rest = &yaml[after_header..];
    let mut offset = 0usize;
    for raw_line in rest.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if (line.starts_with("  /") && line.ends_with(':')) || line == "components:" {
            return Some(&rest[..offset]);
        }
        offset += raw_line.len();
    }
    Some(rest)
}

fn extract_method_block<'a>(path_block: &'a str, method: &str) -> Option<&'a str> {
    let header = format!("    {method}:");
    let (start, _) = exact_line_span(path_block, &header)?;
    let rest = &path_block[start..];
    let mut offset = 0usize;
    for (idx, raw_line) in rest.split_inclusive('\n').enumerate() {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if idx == 0 {
            offset += raw_line.len();
            continue;
        }
        if line.starts_with("    ")
            && !line.starts_with("     ")
            && matches!(
                line[4..].trim_end_matches(':'),
                "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "parameters"
            )
        {
            return Some(&rest[..offset]);
        }
        offset += raw_line.len();
    }
    Some(rest)
}

/// Slice an operation block down to a single status entry's body, e.g. the
/// `"200": ...` sub-block, stopping at the next status key (same 8-space
/// indent, sibling of `responses:`) or any shallower line. Indentation is
/// fixed throughout this file at path(2)/method(4)/`responses:`(6)/status(8).
fn extract_status_block<'a>(op_block: &'a str, status: &str) -> Option<&'a str> {
    let needle = format!("        \"{status}\":");
    let needle_alt = format!("        '{status}':");
    let (start, _) =
        exact_line_span(op_block, &needle).or_else(|| exact_line_span(op_block, &needle_alt))?;
    let rest = &op_block[start..];
    let mut offset = 0usize;
    for (idx, raw_line) in rest.split_inclusive('\n').enumerate() {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if idx == 0 {
            offset += raw_line.len();
            continue;
        }
        if !line.trim().is_empty() {
            let indent = line.len() - line.trim_start().len();
            if indent <= 8 {
                return Some(&rest[..offset]);
            }
        }
        offset += raw_line.len();
    }
    Some(rest)
}

fn exact_line_span(input: &str, expected: &str) -> Option<(usize, usize)> {
    let mut offset = 0usize;
    for raw_line in input.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        let after = offset + raw_line.len();
        if line == expected {
            return Some((offset, after));
        }
        offset = after;
    }
    None
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

    #[test]
    fn openapi_inventory_is_newline_independent() {
        let lf = embedded_openapi_yaml().replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        let gaps = router_openapi_parity_gaps(&crlf);
        assert!(
            gaps.is_empty(),
            "CRLF OpenAPI/router parity gaps: {}",
            gaps.join("; ")
        );
    }

    #[test]
    fn openapi_schema_is_complete() {
        let yaml = embedded_openapi_yaml();
        let gaps = openapi_schema_completeness_gaps(yaml);
        assert!(
            gaps.is_empty(),
            "OpenAPI schema completeness gaps: {}",
            gaps.join("; ")
        );
    }

    #[test]
    fn openapi_schema_completeness_gaps_detects_stripped_request_body() {
        // /auth/login stripped of its requestBody (compare to the real
        // embedded document, which does declare one for this operation).
        let yaml = concat!(
            "  /auth/login:\n",
            "    post:\n",
            "      operationId: authLogin\n",
            "      responses:\n",
            "        \"200\":\n",
            "          description: ok\n",
            "          content:\n",
            "            application/json:\n",
            "              schema: {}\n",
        );
        let gaps = openapi_request_body_gaps(yaml);
        assert!(
            gaps.iter()
                .any(|g| g == "missing requestBody on post /auth/login"),
            "expected missing-requestBody gap, got: {gaps:?}"
        );
        // A fixture that keeps requestBody must not be flagged.
        let fixed = concat!(
            "  /auth/login:\n",
            "    post:\n",
            "      operationId: authLogin\n",
            "      requestBody:\n",
            "        required: true\n",
            "        content:\n",
            "          application/json:\n",
            "            schema: {}\n",
            "      responses:\n",
            "        \"200\":\n",
            "          description: ok\n",
            "          content:\n",
            "            application/json:\n",
            "              schema: {}\n",
        );
        assert!(!openapi_request_body_gaps(fixed)
            .iter()
            .any(|g| g == "missing requestBody on post /auth/login"));
    }

    #[test]
    fn openapi_schema_completeness_gaps_detects_stripped_response_content() {
        // /auth/login's 200 stripped of its content block.
        let yaml = concat!(
            "  /auth/login:\n",
            "    post:\n",
            "      operationId: authLogin\n",
            "      responses:\n",
            "        \"200\":\n",
            "          description: ok, but no content schema\n",
            "        \"401\":\n",
            "          $ref: \"#/components/responses/ApiError\"\n",
        );
        let gaps = openapi_response_content_gaps(yaml);
        assert_eq!(
            gaps,
            vec!["missing response content on 200 for post /auth/login".to_string()],
            "expected exactly one gap for the stripped 200, got: {gaps:?}"
        );
        // A fixture that keeps content must not be flagged.
        let fixed = concat!(
            "  /auth/login:\n",
            "    post:\n",
            "      operationId: authLogin\n",
            "      responses:\n",
            "        \"200\":\n",
            "          description: ok\n",
            "          content:\n",
            "            application/json:\n",
            "              schema: {}\n",
            "        \"401\":\n",
            "          $ref: \"#/components/responses/ApiError\"\n",
        );
        assert!(openapi_response_content_gaps(fixed).is_empty());
    }

    #[test]
    fn method_blocks_do_not_borrow_adjacent_statuses() {
        let yaml = concat!(
            "  /example:\r\n",
            "    get:\r\n",
            "      responses:\r\n",
            "        \"200\": {}\r\n",
            "    post:\r\n",
            "      responses:\r\n",
            "        \"201\": {}\r\n",
        );
        let path = extract_path_block(yaml, "/example").expect("path block");
        let get = extract_method_block(path, "get").expect("GET block");
        let post = extract_method_block(path, "post").expect("POST block");
        assert!(get.contains("\"200\":"));
        assert!(!get.contains("\"201\":"));
        assert!(post.contains("\"201\":"));
    }
}
