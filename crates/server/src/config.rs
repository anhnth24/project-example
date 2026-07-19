//! Typed, fail-fast server configuration with redacted secrets.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;

use serde::Deserialize;

const DEFAULT_MAX_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;
const DEFAULT_JOB_LEASE_SECONDS: u64 = 60;

/// Deployment profile with explicitly different safety requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Dev,
    Test,
    Prod,
}

impl Profile {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dev" => Ok(Self::Dev),
            "test" => Ok(Self::Test),
            "prod" => Ok(Self::Prod),
            _ => Err("MARKHAND_PROFILE must be dev, test, or prod".into()),
        }
    }
}

/// Secret configuration value that cannot leak through Debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub profile: Profile,
    pub bind_addr: SocketAddr,
    pub database_url: Option<SecretString>,
    pub qdrant_url: Option<String>,
    pub minio_url: Option<String>,
    pub auth: AuthConfig,
    pub limits: RuntimeLimits,
    pub index_signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub signing_key: Option<SecretString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub max_upload_bytes: u64,
    pub job_lease_seconds: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigFile {
    profile: Option<String>,
    bind_addr: Option<String>,
    database_url: Option<String>,
    qdrant_url: Option<String>,
    minio_url: Option<String>,
    auth_issuer: Option<String>,
    auth_audience: Option<String>,
    auth_signing_key: Option<String>,
    max_upload_bytes: Option<u64>,
    job_lease_seconds: Option<u64>,
    index_signature: Option<String>,
}

impl ServerConfig {
    /// Precedence: safe defaults < optional JSON config file < environment.
    pub fn from_env() -> Result<Self, String> {
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let file = env
            .get("MARKHAND_CONFIG_FILE")
            .map(Path::new)
            .map(load_file)
            .transpose()?;
        Self::from_sources(file.as_ref(), &env)
    }

    fn from_sources(
        file: Option<&ConfigFile>,
        env: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let profile_raw = env
            .get("MARKHAND_PROFILE")
            .or_else(|| file.and_then(|value| value.profile.as_ref()))
            .map_or("dev", String::as_str);
        let profile = Profile::parse(profile_raw)?;

        let bind_raw = env
            .get("MARKHAND_BIND_ADDR")
            .or_else(|| file.and_then(|value| value.bind_addr.as_ref()))
            .map_or("127.0.0.1:8787", String::as_str);
        let bind_addr = bind_raw
            .parse::<SocketAddr>()
            .map_err(|_| "MARKHAND_BIND_ADDR must be an IP address and port".to_string())?;

        let database_url = env
            .get("MARKHAND_DATABASE_URL")
            .or_else(|| file.and_then(|value| value.database_url.as_ref()))
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .map(SecretString::new);
        let qdrant_url = optional_value(file, env, "MARKHAND_QDRANT_URL", |value| {
            value.qdrant_url.as_ref()
        });
        let minio_url = optional_value(file, env, "MARKHAND_MINIO_URL", |value| {
            value.minio_url.as_ref()
        });
        let auth = AuthConfig {
            issuer: optional_value(file, env, "MARKHAND_AUTH_ISSUER", |value| {
                value.auth_issuer.as_ref()
            }),
            audience: optional_value(file, env, "MARKHAND_AUTH_AUDIENCE", |value| {
                value.auth_audience.as_ref()
            }),
            signing_key: optional_value(file, env, "MARKHAND_AUTH_SIGNING_KEY", |value| {
                value.auth_signing_key.as_ref()
            })
            .map(SecretString::new),
        };
        let limits = RuntimeLimits {
            max_upload_bytes: numeric_value(
                file,
                env,
                "MARKHAND_MAX_UPLOAD_BYTES",
                |value| value.max_upload_bytes,
                DEFAULT_MAX_UPLOAD_BYTES,
            )?,
            job_lease_seconds: numeric_value(
                file,
                env,
                "MARKHAND_JOB_LEASE_SECONDS",
                |value| value.job_lease_seconds,
                DEFAULT_JOB_LEASE_SECONDS,
            )?,
        };
        let index_signature = optional_value(file, env, "MARKHAND_INDEX_SIGNATURE", |value| {
            value.index_signature.as_ref()
        });

        let config = Self {
            profile,
            bind_addr,
            database_url,
            qdrant_url,
            minio_url,
            auth,
            limits,
            index_signature,
        };
        config.validate()?;
        Ok(config)
    }

    /// Returns the service endpoints required to start the real POC server.
    pub fn runtime_endpoints(&self) -> Result<RuntimeEndpoints, String> {
        Ok(RuntimeEndpoints {
            database_url: required_database_url(self.database_url.as_ref())?,
            qdrant_url: required_url(self.qdrant_url.as_deref(), "MARKHAND_QDRANT_URL")?,
            minio_url: required_url(self.minio_url.as_deref(), "MARKHAND_MINIO_URL")?,
        })
    }

    fn validate(&self) -> Result<(), String> {
        self.validate_limits()?;
        self.validate_index_signature(false)?;
        self.validate_auth()?;
        if self.profile != Profile::Prod {
            return Ok(());
        }
        if self.bind_addr.ip().is_loopback() || self.bind_addr.ip().is_unspecified() {
            return Err("prod profile requires a non-loopback explicit bind address".into());
        }
        let endpoints = self.runtime_endpoints()?;
        validate_production_database_url(endpoints.database_url.expose())?;
        require_https(&endpoints.qdrant_url, "MARKHAND_QDRANT_URL")?;
        require_https(&endpoints.minio_url, "MARKHAND_MINIO_URL")?;
        self.validate_index_signature(true)?;
        Ok(())
    }

    fn validate_auth(&self) -> Result<(), String> {
        let Some(issuer) = self.auth.issuer.as_deref() else {
            return if self.profile == Profile::Prod {
                Err("prod profile requires MARKHAND_AUTH_ISSUER".into())
            } else {
                Ok(())
            };
        };
        reqwest::Url::parse(issuer)
            .map_err(|_| "MARKHAND_AUTH_ISSUER must be an absolute URL".to_string())?;
        if self.auth.audience.as_deref().is_none_or(str::is_empty) {
            return Err("MARKHAND_AUTH_AUDIENCE must not be empty when issuer is set".into());
        }
        let Some(signing_key) = self.auth.signing_key.as_ref() else {
            return Err("MARKHAND_AUTH_SIGNING_KEY is required when issuer is set".into());
        };
        if signing_key.expose().len() < 32 {
            return Err("MARKHAND_AUTH_SIGNING_KEY must contain at least 32 bytes".into());
        }
        Ok(())
    }

    fn validate_limits(&self) -> Result<(), String> {
        if self.limits.max_upload_bytes == 0
            || self.limits.max_upload_bytes > DEFAULT_MAX_UPLOAD_BYTES
        {
            return Err(format!(
                "MARKHAND_MAX_UPLOAD_BYTES must be between 1 and {DEFAULT_MAX_UPLOAD_BYTES}"
            ));
        }
        if self.limits.job_lease_seconds == 0 || self.limits.job_lease_seconds > 3600 {
            return Err("MARKHAND_JOB_LEASE_SECONDS must be between 1 and 3600".into());
        }
        Ok(())
    }

    fn validate_index_signature(&self, required: bool) -> Result<(), String> {
        let Some(signature) = self.index_signature.as_deref() else {
            return if required {
                Err("prod profile requires MARKHAND_INDEX_SIGNATURE".into())
            } else {
                Ok(())
            };
        };
        if signature.len() != 64 || !signature.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("MARKHAND_INDEX_SIGNATURE must be a 64-character hex digest".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEndpoints {
    pub database_url: SecretString,
    pub qdrant_url: String,
    pub minio_url: String,
}

fn required_database_url(value: Option<&SecretString>) -> Result<SecretString, String> {
    let value = value.ok_or_else(|| "server requires MARKHAND_DATABASE_URL".to_string())?;
    let parsed = reqwest::Url::parse(value.expose())
        .map_err(|_| "MARKHAND_DATABASE_URL must be an absolute URL".to_string())?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") || parsed.host_str().is_none() {
        return Err("MARKHAND_DATABASE_URL must include a postgres host".into());
    }
    Ok(value.clone())
}

fn optional_value(
    file: Option<&ConfigFile>,
    env: &BTreeMap<String, String>,
    env_name: &str,
    from_file: impl FnOnce(&ConfigFile) -> Option<&String>,
) -> Option<String> {
    env.get(env_name)
        .or_else(|| file.and_then(from_file))
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn numeric_value(
    file: Option<&ConfigFile>,
    env: &BTreeMap<String, String>,
    env_name: &str,
    from_file: impl FnOnce(&ConfigFile) -> Option<u64>,
    default: u64,
) -> Result<u64, String> {
    match env.get(env_name) {
        Some(value) => value
            .parse()
            .map_err(|_| format!("{env_name} must be an unsigned integer")),
        None => Ok(file.and_then(from_file).unwrap_or(default)),
    }
}

fn required_url(value: Option<&str>, name: &str) -> Result<String, String> {
    let value = value.ok_or_else(|| format!("server requires {name}"))?;
    let parsed =
        reqwest::Url::parse(value).map_err(|_| format!("{name} must be an absolute URL"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("{name} must use http or https"));
    }
    Ok(value.trim_end_matches('/').to_string())
}

fn validate_production_database_url(value: &str) -> Result<(), String> {
    let parsed =
        reqwest::Url::parse(value).map_err(|_| "MARKHAND_DATABASE_URL must be an absolute URL")?;
    if !matches!(parsed.scheme(), "postgres" | "postgresql") {
        return Err("MARKHAND_DATABASE_URL must use postgres or postgresql".into());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "MARKHAND_DATABASE_URL must include a host".to_string())?;
    if host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || (parsed.username() == "markhand" && parsed.password() == Some("markhand_dev_only"))
        || (parsed.username() == "postgres" && parsed.password() == Some("postgres"))
        || parsed.password() == Some("markhand_dev_only")
    {
        return Err("prod profile cannot use a development database URL".into());
    }
    let sslmode = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "sslmode").then_some(value));
    if sslmode.as_deref() != Some("require") {
        return Err("prod MARKHAND_DATABASE_URL requires sslmode=require".into());
    }
    Ok(())
}

fn require_https(value: &str, name: &str) -> Result<(), String> {
    if reqwest::Url::parse(value)
        .ok()
        .is_some_and(|parsed| parsed.scheme() == "https")
    {
        Ok(())
    } else {
        Err(format!("prod {name} must use https"))
    }
}

fn load_file(path: &Path) -> Result<ConfigFile, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|_| "cannot read MARKHAND_CONFIG_FILE".to_string())?;
    serde_json::from_str(&source).map_err(|_| "MARKHAND_CONFIG_FILE contains invalid JSON".into())
}

#[cfg(test)]
mod tests {
    use super::{ConfigFile, Profile, ServerConfig};
    use std::collections::BTreeMap;

    #[test]
    fn environment_overrides_file_and_defaults() {
        let file = ConfigFile {
            profile: Some("test".into()),
            bind_addr: Some("127.0.0.1:9000".into()),
            database_url: Some("postgres://file-secret".into()),
            qdrant_url: Some("http://qdrant.test".into()),
            minio_url: Some("http://minio.test".into()),
            auth_issuer: None,
            auth_audience: None,
            auth_signing_key: None,
            max_upload_bytes: None,
            job_lease_seconds: None,
            index_signature: None,
        };
        let env = BTreeMap::from([
            ("MARKHAND_PROFILE".into(), "dev".into()),
            ("MARKHAND_BIND_ADDR".into(), "127.0.0.1:9010".into()),
        ]);
        let config = ServerConfig::from_sources(Some(&file), &env).unwrap();
        assert_eq!(config.profile, Profile::Dev);
        assert_eq!(config.bind_addr.to_string(), "127.0.0.1:9010");
        assert_eq!(
            config.database_url.unwrap().expose(),
            "postgres://file-secret"
        );
        assert_eq!(config.qdrant_url.as_deref(), Some("http://qdrant.test"));
    }

    #[test]
    fn secret_is_redacted_and_prod_rejects_dev_values() {
        let env = BTreeMap::from([
            ("MARKHAND_PROFILE".into(), "prod".into()),
            ("MARKHAND_BIND_ADDR".into(), "127.0.0.1:8787".into()),
            (
                "MARKHAND_AUTH_ISSUER".into(),
                "https://issuer.example.test".into(),
            ),
            ("MARKHAND_AUTH_AUDIENCE".into(), "markhand-api".into()),
            (
                "MARKHAND_AUTH_SIGNING_KEY".into(),
                "this-test-signing-key-is-at-least-32-bytes".into(),
            ),
            (
                "MARKHAND_DATABASE_URL".into(),
                "postgres://postgres:postgres@localhost/db".into(),
            ),
        ]);
        let error = ServerConfig::from_sources(None, &env).unwrap_err();
        assert!(error.contains("bind address"));

        let config = ServerConfig::from_sources(
            None,
            &BTreeMap::from([("MARKHAND_DATABASE_URL".into(), "top-secret".into())]),
        )
        .unwrap();
        assert!(!format!("{config:?}").contains("top-secret"));
    }

    #[test]
    fn invalid_bind_fails_fast() {
        let env = BTreeMap::from([("MARKHAND_BIND_ADDR".into(), "not-an-address".into())]);
        assert!(ServerConfig::from_sources(None, &env).is_err());
    }

    #[test]
    fn runtime_endpoints_require_all_real_services() {
        let config = ServerConfig::from_sources(
            None,
            &BTreeMap::from([(
                "MARKHAND_DATABASE_URL".into(),
                "postgres://markhand@localhost/markhand".into(),
            )]),
        )
        .unwrap();
        assert_eq!(
            config.runtime_endpoints().unwrap_err(),
            "server requires MARKHAND_QDRANT_URL"
        );
    }

    #[test]
    fn production_requires_verified_dependency_transport() {
        let env = BTreeMap::from([
            ("MARKHAND_PROFILE".into(), "prod".into()),
            ("MARKHAND_BIND_ADDR".into(), "10.0.0.10:8787".into()),
            (
                "MARKHAND_DATABASE_URL".into(),
                "postgres://app:secret@postgres.internal/markhand?sslmode=require".into(),
            ),
            (
                "MARKHAND_QDRANT_URL".into(),
                "https://qdrant.internal".into(),
            ),
            ("MARKHAND_MINIO_URL".into(), "http://minio.internal".into()),
            (
                "MARKHAND_AUTH_ISSUER".into(),
                "https://auth.internal".into(),
            ),
            ("MARKHAND_AUTH_AUDIENCE".into(), "markhand-web".into()),
            (
                "MARKHAND_AUTH_SIGNING_KEY".into(),
                "0123456789abcdef0123456789abcdef".into(),
            ),
            (
                "MARKHAND_INDEX_SIGNATURE".into(),
                "d54db7b6de20b51a416670927eeab346256c9b891732965e51586fac333c1835".into(),
            ),
        ]);
        assert_eq!(
            ServerConfig::from_sources(None, &env).unwrap_err(),
            "prod MARKHAND_MINIO_URL must use https"
        );
    }

    #[test]
    fn auth_and_limit_configuration_fail_fast() {
        let invalid_limit =
            BTreeMap::from([("MARKHAND_MAX_UPLOAD_BYTES".into(), "not-a-number".into())]);
        assert_eq!(
            ServerConfig::from_sources(None, &invalid_limit).unwrap_err(),
            "MARKHAND_MAX_UPLOAD_BYTES must be an unsigned integer"
        );

        let incomplete_auth = BTreeMap::from([(
            "MARKHAND_AUTH_ISSUER".into(),
            "https://markhand.test".into(),
        )]);
        assert_eq!(
            ServerConfig::from_sources(None, &incomplete_auth).unwrap_err(),
            "MARKHAND_AUTH_AUDIENCE must not be empty when issuer is set"
        );

        let invalid_signature =
            BTreeMap::from([("MARKHAND_INDEX_SIGNATURE".into(), "not-a-signature".into())]);
        assert_eq!(
            ServerConfig::from_sources(None, &invalid_signature).unwrap_err(),
            "MARKHAND_INDEX_SIGNATURE must be a 64-character hex digest"
        );
    }
}
