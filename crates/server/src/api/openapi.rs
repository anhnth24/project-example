//! Embedded OpenAPI helpers (P1B-R06).

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_covers_phase1b_routes() {
        let yaml = embedded_openapi_yaml();
        for path in [
            "/health/live",
            "/health/ready",
            "/auth/login",
            "/auth/refresh",
            "/uploads",
            "/collections",
            "/citations/resolve",
            "/conflicts",
            "/conflicts/{conflictId}/evidence",
            "/jobs/{jobId}",
            "/jobs/{jobId}/events",
            "/search",
            "/ask",
            "/ask/stream",
            "/openapi.yaml",
        ] {
            assert!(yaml.contains(path), "openapi missing path fragment {path}");
        }
        assert!(yaml.contains("RateLimited") || yaml.contains("rate_limited"));
        assert!(yaml.contains("sourceContentSha256"));
        assert!(yaml.contains("canonicalMarkdownSha256"));
        assert!(!yaml.contains("contentSha256:"));
        assert!(openapi_path_count() >= 20);
    }
}
