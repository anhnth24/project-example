//! Reject credentialed or parameterized service base URLs (fail closed).
//!
//! Service base URLs must be `scheme://host[:port][/path]` only — no userinfo,
//! query, or fragment.

use crate::storage::error::StorageError;

/// Normalize an absolute http(s) base URL and reject credentials / query / fragment.
pub fn normalize_service_url(url: impl AsRef<str>) -> Result<String, StorageError> {
    let trimmed = url.as_ref().trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(StorageError::ConfigInvalid);
    }
    // Fail closed on raw markers before parse edge cases.
    if trimmed.contains('?') || trimmed.contains('#') {
        return Err(StorageError::ConfigInvalid);
    }
    let parsed = reqwest::Url::parse(trimmed).map_err(|_| StorageError::ConfigInvalid)?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(StorageError::ConfigInvalid);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(StorageError::ConfigInvalid);
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(StorageError::ConfigInvalid);
    }
    if parsed.host_str().is_none() {
        return Err(StorageError::ConfigInvalid);
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_userinfo_query_and_fragment() {
        assert!(normalize_service_url("http://user:pass@127.0.0.1:9000").is_err());
        assert!(normalize_service_url("http://127.0.0.1:9000?api_key=secret").is_err());
        assert!(normalize_service_url("http://127.0.0.1:9000?sig=abc").is_err());
        assert!(normalize_service_url("http://127.0.0.1:9000#frag").is_err());
        assert!(normalize_service_url("http://127.0.0.1:9000/?x=1").is_err());
        assert_eq!(
            normalize_service_url("http://127.0.0.1:9000/").unwrap(),
            "http://127.0.0.1:9000"
        );
        assert_eq!(
            normalize_service_url("http://127.0.0.1:9000/minio").unwrap(),
            "http://127.0.0.1:9000/minio"
        );
    }
}
