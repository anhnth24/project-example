//! Typed, fail-fast server configuration with redacted secrets.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;

use serde::Deserialize;

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
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigFile {
    profile: Option<String>,
    bind_addr: Option<String>,
    database_url: Option<String>,
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
            .map(SecretString);

        let config = Self {
            profile,
            bind_addr,
            database_url,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if self.profile != Profile::Prod {
            return Ok(());
        }
        if self.bind_addr.ip().is_loopback() || self.bind_addr.ip().is_unspecified() {
            return Err("prod profile requires a non-loopback explicit bind address".into());
        }
        let Some(database_url) = self.database_url.as_ref() else {
            return Err("prod profile requires MARKHAND_DATABASE_URL".into());
        };
        let value = database_url.expose().to_ascii_lowercase();
        if value.contains("localhost") || value.contains("postgres:postgres") {
            return Err("prod profile cannot use a development database URL".into());
        }
        Ok(())
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
    }

    #[test]
    fn secret_is_redacted_and_prod_rejects_dev_values() {
        let env = BTreeMap::from([
            ("MARKHAND_PROFILE".into(), "prod".into()),
            ("MARKHAND_BIND_ADDR".into(), "127.0.0.1:8787".into()),
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
}
