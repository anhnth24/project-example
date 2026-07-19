//! Typed, fail-fast server configuration with redacted secrets.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::Path;

use serde::Deserialize;

use crate::services::upload::{LimitsConfig, UploadConfig};

const DEFAULT_MAX_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;
const DEFAULT_JOB_LEASE_SECONDS: u64 = 60;
const DEFAULT_MINIO_OPERATION_TIMEOUT_SECS: u64 = 30;
const MAX_MINIO_OPERATION_TIMEOUT_SECS: u64 = 300;
const DEFAULT_ACCESS_TOKEN_TTL_SECS: u64 = 900;
const DEFAULT_REFRESH_TOKEN_TTL_SECS: u64 = 60 * 60 * 24 * 7;
const DEFAULT_ARGON2_MEMORY_KIB: u32 = 19_456;
const DEFAULT_ARGON2_TIME_COST: u32 = 2;
const DEFAULT_ARGON2_PARALLELISM: u32 = 1;
const PROD_MIN_ARGON2_MEMORY_KIB: u32 = 19_456;
const PROD_MIN_ARGON2_TIME_COST: u32 = 2;
const PROD_MAX_ACCESS_TOKEN_TTL_SECS: u64 = 900;

/// Deployment profile with explicitly different safety requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Dev,
    Test,
    Prod,
}

/// Process role controls which secrets and invariants are applicable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeRole {
    Api,
    Worker,
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

#[derive(Clone, PartialEq, Eq)]
pub struct ServerConfig {
    role: RuntimeRole,
    profile: Profile,
    bind_addr: SocketAddr,
    database_url: Option<SecretString>,
    qdrant_url: Option<String>,
    qdrant_api_key: Option<SecretString>,
    minio_url: Option<String>,
    minio_access_key: Option<SecretString>,
    minio_secret_key: Option<SecretString>,
    minio_bucket: Option<String>,
    minio_region: Option<String>,
    minio_path_style: bool,
    minio_operation_timeout_secs: u64,
    auth: AuthConfig,
    limits: RuntimeLimits,
    upload: UploadConfig,
    index_signature: Option<String>,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServerConfig")
            .field("role", &self.role)
            .field("profile", &self.profile)
            .field("bind_addr", &self.bind_addr)
            .field(
                "database_url",
                &self.database_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "qdrant_url",
                &self.qdrant_url.as_ref().map(|_| "[REDACTED_ENDPOINT]"),
            )
            .field(
                "qdrant_api_key",
                &self.qdrant_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "minio_url",
                &self.minio_url.as_ref().map(|_| "[REDACTED_ENDPOINT]"),
            )
            .field(
                "minio_access_key",
                &self.minio_access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "minio_secret_key",
                &self.minio_secret_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("minio_bucket", &self.minio_bucket)
            .field("minio_region", &self.minio_region)
            .field("minio_path_style", &self.minio_path_style)
            .field(
                "minio_operation_timeout_secs",
                &self.minio_operation_timeout_secs,
            )
            .field("auth", &self.auth)
            .field("limits", &self.limits)
            .field("upload", &self.upload)
            .field("index_signature", &self.index_signature)
            .finish()
    }
}

/// Typed MinIO/S3 connection settings (credentials redacted in Debug).
#[derive(Clone, PartialEq, Eq)]
pub struct MinioConfig {
    endpoint: String,
    access_key: SecretString,
    secret_key: SecretString,
    bucket: String,
    region: String,
    path_style: bool,
    operation_timeout_secs: u64,
}

impl fmt::Debug for MinioConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MinioConfig")
            .field("endpoint", &"[REDACTED_ENDPOINT]")
            .field("access_key", &self.access_key)
            .field("secret_key", &self.secret_key)
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("path_style", &self.path_style)
            .field("operation_timeout_secs", &self.operation_timeout_secs)
            .finish()
    }
}

impl MinioConfig {
    pub fn new(
        endpoint: impl Into<String>,
        access_key: SecretString,
        secret_key: SecretString,
        bucket: impl Into<String>,
        region: impl Into<String>,
        path_style: bool,
    ) -> Result<Self, String> {
        Self::new_with_timeout(
            endpoint,
            access_key,
            secret_key,
            bucket,
            region,
            path_style,
            DEFAULT_MINIO_OPERATION_TIMEOUT_SECS,
        )
    }

    pub fn new_with_timeout(
        endpoint: impl Into<String>,
        access_key: SecretString,
        secret_key: SecretString,
        bucket: impl Into<String>,
        region: impl Into<String>,
        path_style: bool,
        operation_timeout_secs: u64,
    ) -> Result<Self, String> {
        let endpoint_raw = endpoint.into();
        let endpoint = required_url(Some(endpoint_raw.as_str()), "MARKHAND_MINIO_URL")?;
        let access_key_raw = access_key.expose();
        let secret_key_raw = secret_key.expose();
        if access_key_raw.is_empty() || secret_key_raw.is_empty() {
            return Err("MinIO access key and secret key must not be empty".into());
        }
        let bucket = bucket.into();
        if bucket.trim().is_empty() {
            return Err("MARKHAND_MINIO_BUCKET must not be empty".into());
        }
        let region = region.into();
        if region.trim().is_empty() {
            return Err("MARKHAND_MINIO_REGION must not be empty".into());
        }
        if operation_timeout_secs == 0 || operation_timeout_secs > MAX_MINIO_OPERATION_TIMEOUT_SECS
        {
            return Err(format!(
                "MARKHAND_MINIO_OPERATION_TIMEOUT_SECS must be between 1 and {MAX_MINIO_OPERATION_TIMEOUT_SECS}"
            ));
        }
        Ok(Self {
            endpoint,
            access_key,
            secret_key,
            bucket,
            region,
            path_style,
            operation_timeout_secs,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn access_key(&self) -> &SecretString {
        &self.access_key
    }

    pub fn secret_key(&self) -> &SecretString {
        &self.secret_key
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub const fn path_style(&self) -> bool {
        self.path_style
    }

    pub const fn operation_timeout_secs(&self) -> u64 {
        self.operation_timeout_secs
    }
}

/// Fail-closed storage adapter configuration (Qdrant + MinIO).
#[derive(Clone, PartialEq, Eq)]
pub struct StorageConfig {
    qdrant_url: String,
    qdrant_api_key: Option<SecretString>,
    minio: MinioConfig,
}

impl fmt::Debug for StorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StorageConfig")
            .field("qdrant_url", &"[REDACTED_ENDPOINT]")
            .field(
                "qdrant_api_key",
                &self.qdrant_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("minio", &self.minio)
            .finish()
    }
}

impl StorageConfig {
    pub fn qdrant_url(&self) -> &str {
        &self.qdrant_url
    }

    pub fn qdrant_api_key(&self) -> Option<&SecretString> {
        self.qdrant_api_key.as_ref()
    }

    pub fn minio(&self) -> &MinioConfig {
        &self.minio
    }
}

/// Pinned JWT signing algorithm. Only HS256 is supported for Phase 1B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtAlgorithm {
    Hs256,
}

impl JwtAlgorithm {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_uppercase().as_str() {
            "HS256" => Ok(Self::Hs256),
            _ => Err("MARKHAND_AUTH_ALG must be HS256".into()),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hs256 => "HS256",
        }
    }
}

/// Argon2id cost parameters (pinned in config; rehash-on-login when raised).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argon2Config {
    pub memory_kib: u32,
    pub time_cost: u32,
    pub parallelism: u32,
}

impl Argon2Config {
    pub const fn defaults() -> Self {
        Self {
            memory_kib: DEFAULT_ARGON2_MEMORY_KIB,
            time_cost: DEFAULT_ARGON2_TIME_COST,
            parallelism: DEFAULT_ARGON2_PARALLELISM,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub signing_key: Option<SecretString>,
    pub alg: JwtAlgorithm,
    pub kid: Option<String>,
    pub access_token_ttl_secs: u64,
    pub refresh_token_ttl_secs: u64,
    pub argon2: Argon2Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub max_upload_bytes: u64,
    pub job_lease_seconds: u64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigFile {
    profile: Option<String>,
    bind_addr: Option<String>,
    database_url: Option<String>,
    qdrant_url: Option<String>,
    qdrant_api_key: Option<String>,
    minio_url: Option<String>,
    minio_access_key: Option<String>,
    minio_secret_key: Option<String>,
    minio_bucket: Option<String>,
    minio_region: Option<String>,
    minio_path_style: Option<bool>,
    minio_operation_timeout_secs: Option<u64>,
    auth_issuer: Option<String>,
    auth_audience: Option<String>,
    auth_signing_key: Option<String>,
    auth_alg: Option<String>,
    auth_kid: Option<String>,
    auth_access_token_ttl_secs: Option<u64>,
    auth_refresh_token_ttl_secs: Option<u64>,
    auth_argon2_memory_kib: Option<u64>,
    auth_argon2_time_cost: Option<u64>,
    auth_argon2_parallelism: Option<u64>,
    max_upload_bytes: Option<u64>,
    job_lease_seconds: Option<u64>,
    max_archive_entries: Option<u64>,
    max_archive_uncompressed_bytes: Option<u64>,
    max_archive_compression_ratio: Option<u64>,
    max_pdf_pages: Option<u64>,
    max_image_pixels: Option<u64>,
    max_audio_duration_secs: Option<u64>,
    max_multipart_parts: Option<u64>,
    max_part_header_bytes: Option<u64>,
    upload_timeout_secs: Option<u64>,
    upload_idle_timeout_secs: Option<u64>,
    index_signature: Option<String>,
}

impl ServerConfig {
    /// Precedence: safe defaults < optional JSON config file < environment.
    pub fn from_env() -> Result<Self, String> {
        Self::from_env_for_role(RuntimeRole::Api)
    }

    /// Loads the worker configuration without reading API authentication secrets.
    pub fn from_worker_env() -> Result<Self, String> {
        Self::from_env_for_role(RuntimeRole::Worker)
    }

    fn from_env_for_role(role: RuntimeRole) -> Result<Self, String> {
        let env: BTreeMap<String, String> = std::env::vars().collect();
        if role == RuntimeRole::Worker {
            reject_worker_auth_environment(&env)?;
        }
        let file = env
            .get("MARKHAND_CONFIG_FILE")
            .map(Path::new)
            .map(|path| load_file(path, role))
            .transpose()?;
        Self::from_sources_for_role(file.as_ref(), &env, role)
    }

    #[cfg(test)]
    fn from_sources(
        file: Option<&ConfigFile>,
        env: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        Self::from_sources_for_role(file, env, RuntimeRole::Api)
    }

    fn from_sources_for_role(
        file: Option<&ConfigFile>,
        env: &BTreeMap<String, String>,
        role: RuntimeRole,
    ) -> Result<Self, String> {
        if role == RuntimeRole::Worker {
            reject_worker_auth_environment(env)?;
        }
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
        let qdrant_api_key = optional_value(file, env, "MARKHAND_QDRANT_API_KEY", |value| {
            value.qdrant_api_key.as_ref()
        })
        .map(SecretString::new);
        let minio_url = optional_value(file, env, "MARKHAND_MINIO_URL", |value| {
            value.minio_url.as_ref()
        });
        let minio_access_key = optional_value(file, env, "MARKHAND_MINIO_ACCESS_KEY", |value| {
            value.minio_access_key.as_ref()
        })
        .map(SecretString::new);
        let minio_secret_key = optional_value(file, env, "MARKHAND_MINIO_SECRET_KEY", |value| {
            value.minio_secret_key.as_ref()
        })
        .map(SecretString::new);
        let minio_bucket = optional_value(file, env, "MARKHAND_MINIO_BUCKET", |value| {
            value.minio_bucket.as_ref()
        });
        let minio_region = optional_value(file, env, "MARKHAND_MINIO_REGION", |value| {
            value.minio_region.as_ref()
        });
        let minio_path_style = bool_value(
            file,
            env,
            "MARKHAND_MINIO_PATH_STYLE",
            |value| value.minio_path_style,
            true,
        )?;
        let minio_operation_timeout_secs = numeric_value(
            file,
            env,
            "MARKHAND_MINIO_OPERATION_TIMEOUT_SECS",
            |value| value.minio_operation_timeout_secs,
            DEFAULT_MINIO_OPERATION_TIMEOUT_SECS,
        )?;
        let auth = match role {
            RuntimeRole::Api => {
                let alg = optional_value(file, env, "MARKHAND_AUTH_ALG", |value| {
                    value.auth_alg.as_ref()
                })
                .as_deref()
                .map(JwtAlgorithm::parse)
                .transpose()?
                .unwrap_or(JwtAlgorithm::Hs256);
                let argon2 = Argon2Config {
                    memory_kib: u32_value(
                        file,
                        env,
                        "MARKHAND_AUTH_ARGON2_MEMORY_KIB",
                        |value| value.auth_argon2_memory_kib,
                        DEFAULT_ARGON2_MEMORY_KIB,
                    )?,
                    time_cost: u32_value(
                        file,
                        env,
                        "MARKHAND_AUTH_ARGON2_TIME_COST",
                        |value| value.auth_argon2_time_cost,
                        DEFAULT_ARGON2_TIME_COST,
                    )?,
                    parallelism: u32_value(
                        file,
                        env,
                        "MARKHAND_AUTH_ARGON2_PARALLELISM",
                        |value| value.auth_argon2_parallelism,
                        DEFAULT_ARGON2_PARALLELISM,
                    )?,
                };
                AuthConfig {
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
                    alg,
                    kid: optional_value(file, env, "MARKHAND_AUTH_KID", |value| {
                        value.auth_kid.as_ref()
                    }),
                    access_token_ttl_secs: numeric_value(
                        file,
                        env,
                        "MARKHAND_AUTH_ACCESS_TOKEN_TTL_SECS",
                        |value| value.auth_access_token_ttl_secs,
                        DEFAULT_ACCESS_TOKEN_TTL_SECS,
                    )?,
                    refresh_token_ttl_secs: numeric_value(
                        file,
                        env,
                        "MARKHAND_AUTH_REFRESH_TOKEN_TTL_SECS",
                        |value| value.auth_refresh_token_ttl_secs,
                        DEFAULT_REFRESH_TOKEN_TTL_SECS,
                    )?,
                    argon2,
                }
            }
            RuntimeRole::Worker => AuthConfig {
                issuer: None,
                audience: None,
                signing_key: None,
                alg: JwtAlgorithm::Hs256,
                kid: None,
                access_token_ttl_secs: DEFAULT_ACCESS_TOKEN_TTL_SECS,
                refresh_token_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
                argon2: Argon2Config::defaults(),
            },
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
        let upload_limits = LimitsConfig {
            max_upload_bytes: limits.max_upload_bytes,
            max_archive_entries: numeric_value(
                file,
                env,
                "MARKHAND_MAX_ARCHIVE_ENTRIES",
                |value| value.max_archive_entries,
                LimitsConfig::policy_defaults().max_archive_entries,
            )?,
            max_archive_uncompressed_bytes: numeric_value(
                file,
                env,
                "MARKHAND_MAX_ARCHIVE_UNCOMPRESSED_BYTES",
                |value| value.max_archive_uncompressed_bytes,
                LimitsConfig::policy_defaults().max_archive_uncompressed_bytes,
            )?,
            max_archive_compression_ratio: numeric_value(
                file,
                env,
                "MARKHAND_MAX_ARCHIVE_COMPRESSION_RATIO",
                |value| value.max_archive_compression_ratio,
                LimitsConfig::policy_defaults().max_archive_compression_ratio,
            )?,
            max_pdf_pages: u32_value(
                file,
                env,
                "MARKHAND_MAX_PDF_PAGES",
                |value| value.max_pdf_pages,
                LimitsConfig::policy_defaults().max_pdf_pages,
            )?,
            max_image_pixels: numeric_value(
                file,
                env,
                "MARKHAND_MAX_IMAGE_PIXELS",
                |value| value.max_image_pixels,
                LimitsConfig::policy_defaults().max_image_pixels,
            )?,
            max_audio_duration_secs: numeric_value(
                file,
                env,
                "MARKHAND_MAX_AUDIO_DURATION_SECS",
                |value| value.max_audio_duration_secs,
                LimitsConfig::policy_defaults().max_audio_duration_secs,
            )?,
            max_multipart_parts: u32_value(
                file,
                env,
                "MARKHAND_MAX_MULTIPART_PARTS",
                |value| value.max_multipart_parts,
                LimitsConfig::policy_defaults().max_multipart_parts,
            )?,
            max_part_header_bytes: usize_value(
                file,
                env,
                "MARKHAND_MAX_PART_HEADER_BYTES",
                |value| value.max_part_header_bytes,
                LimitsConfig::policy_defaults().max_part_header_bytes,
            )?,
            upload_timeout_secs: numeric_value(
                file,
                env,
                "MARKHAND_UPLOAD_TIMEOUT_SECS",
                |value| value.upload_timeout_secs,
                LimitsConfig::policy_defaults().upload_timeout_secs,
            )?,
            upload_idle_timeout_secs: numeric_value(
                file,
                env,
                "MARKHAND_UPLOAD_IDLE_TIMEOUT_SECS",
                |value| value.upload_idle_timeout_secs,
                LimitsConfig::policy_defaults().upload_idle_timeout_secs,
            )?,
        };
        let upload = UploadConfig {
            limits: upload_limits,
        };
        let index_signature = optional_value(file, env, "MARKHAND_INDEX_SIGNATURE", |value| {
            value.index_signature.as_ref()
        });

        let config = Self {
            role,
            profile,
            bind_addr,
            database_url,
            qdrant_url,
            qdrant_api_key,
            minio_url,
            minio_access_key,
            minio_secret_key,
            minio_bucket,
            minio_region,
            minio_path_style,
            minio_operation_timeout_secs,
            auth,
            limits,
            upload,
            index_signature,
        };
        config.validate()?;
        Ok(config)
    }

    pub const fn profile(&self) -> Profile {
        self.profile
    }

    pub const fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    /// Authentication configuration (API role). Worker configs leave credentials unset.
    pub const fn auth(&self) -> &AuthConfig {
        &self.auth
    }

    /// Upload intake limits (P0-09 policy defaults unless overridden).
    pub const fn upload(&self) -> &UploadConfig {
        &self.upload
    }

    /// Legacy runtime limits (max upload + job lease).
    pub const fn limits(&self) -> RuntimeLimits {
        self.limits
    }

    pub(crate) fn is_api_role(&self) -> bool {
        self.role == RuntimeRole::Api
    }

    /// Test helper for integration crates (`tests/*.rs`) and in-crate HTTP tests.
    pub fn test_with_endpoints(endpoints: RuntimeEndpoints) -> Self {
        Self::test_with_endpoints_for_role(endpoints, RuntimeRole::Api)
    }

    /// Test helper for worker-role runtime state.
    pub fn test_worker_with_endpoints(endpoints: RuntimeEndpoints) -> Self {
        Self::test_with_endpoints_for_role(endpoints, RuntimeRole::Worker)
    }

    fn test_with_endpoints_for_role(endpoints: RuntimeEndpoints, role: RuntimeRole) -> Self {
        Self {
            role,
            profile: Profile::Dev,
            bind_addr: "127.0.0.1:8787".parse().expect("valid test address"),
            database_url: Some(endpoints.database_url),
            qdrant_url: Some(endpoints.qdrant_url),
            qdrant_api_key: None,
            minio_url: Some(endpoints.minio_url),
            minio_access_key: None,
            minio_secret_key: None,
            minio_bucket: None,
            minio_region: None,
            minio_path_style: true,
            minio_operation_timeout_secs: DEFAULT_MINIO_OPERATION_TIMEOUT_SECS,
            auth: AuthConfig {
                issuer: None,
                audience: None,
                signing_key: None,
                alg: JwtAlgorithm::Hs256,
                kid: None,
                access_token_ttl_secs: DEFAULT_ACCESS_TOKEN_TTL_SECS,
                refresh_token_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
                argon2: Argon2Config::defaults(),
            },
            limits: RuntimeLimits {
                max_upload_bytes: DEFAULT_MAX_UPLOAD_BYTES,
                job_lease_seconds: DEFAULT_JOB_LEASE_SECONDS,
            },
            upload: UploadConfig::policy_defaults(),
            index_signature: None,
        }
    }

    /// Returns the service endpoints required to start the real POC server.
    pub fn runtime_endpoints(&self) -> Result<RuntimeEndpoints, String> {
        Ok(RuntimeEndpoints {
            database_url: required_database_url(self.database_url.as_ref())?,
            qdrant_url: required_url(self.qdrant_url.as_deref(), "MARKHAND_QDRANT_URL")?,
            minio_url: required_url(self.minio_url.as_deref(), "MARKHAND_MINIO_URL")?,
        })
    }

    /// Builds typed storage adapter config (Qdrant URL + MinIO credentials).
    ///
    /// Fail-closed: missing endpoint or credentials returns an error. Secrets
    /// remain wrapped in [`SecretString`] (redacted in Debug).
    pub fn storage_config(&self) -> Result<StorageConfig, String> {
        let endpoints = self.runtime_endpoints()?;
        let access_key = self
            .minio_access_key
            .clone()
            .ok_or_else(|| "server requires MARKHAND_MINIO_ACCESS_KEY".to_string())?;
        let secret_key = self
            .minio_secret_key
            .clone()
            .ok_or_else(|| "server requires MARKHAND_MINIO_SECRET_KEY".to_string())?;
        let bucket = self
            .minio_bucket
            .clone()
            .unwrap_or_else(|| "markhand-documents".to_string());
        let region = self
            .minio_region
            .clone()
            .unwrap_or_else(|| "us-east-1".to_string());
        let minio = MinioConfig::new_with_timeout(
            endpoints.minio_url,
            access_key,
            secret_key,
            bucket,
            region,
            self.minio_path_style,
            self.minio_operation_timeout_secs,
        )?;
        Ok(StorageConfig {
            qdrant_url: endpoints.qdrant_url,
            qdrant_api_key: self.qdrant_api_key.clone(),
            minio,
        })
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        self.validate_limits()?;
        self.upload.validate()?;
        self.validate_index_signature(false)?;
        if self.role == RuntimeRole::Api {
            self.validate_auth()?;
        }
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
        // Fail closed: production must have least-privilege MinIO credentials.
        let _ = self.storage_config()?;
        self.validate_index_signature(true)?;
        Ok(())
    }

    fn validate_auth(&self) -> Result<(), String> {
        let Some(issuer) = self.auth.issuer.as_deref() else {
            if self.auth.audience.is_some()
                || self.auth.signing_key.is_some()
                || self.auth.kid.is_some()
            {
                return Err(
                    "MARKHAND_AUTH_ISSUER is required when authentication is configured".into(),
                );
            }
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
        if self.auth.alg != JwtAlgorithm::Hs256 {
            return Err("MARKHAND_AUTH_ALG must be HS256".into());
        }
        let Some(kid) = self.auth.kid.as_deref() else {
            return Err("MARKHAND_AUTH_KID is required when issuer is set".into());
        };
        if kid.trim().is_empty() {
            return Err("MARKHAND_AUTH_KID must not be empty when issuer is set".into());
        }
        if self.auth.access_token_ttl_secs == 0
            || self.auth.access_token_ttl_secs > DEFAULT_ACCESS_TOKEN_TTL_SECS
        {
            return Err(format!(
                "MARKHAND_AUTH_ACCESS_TOKEN_TTL_SECS must be between 1 and {DEFAULT_ACCESS_TOKEN_TTL_SECS}"
            ));
        }
        if self.auth.refresh_token_ttl_secs == 0 {
            return Err("MARKHAND_AUTH_REFRESH_TOKEN_TTL_SECS must be at least 1".into());
        }
        if self.auth.argon2.memory_kib == 0
            || self.auth.argon2.time_cost == 0
            || self.auth.argon2.parallelism == 0
        {
            return Err("Argon2 parameters must be positive".into());
        }
        if self.profile == Profile::Prod {
            if kid.eq_ignore_ascii_case("dev")
                || kid.to_ascii_lowercase().starts_with("dev-")
                || kid.to_ascii_lowercase().starts_with("test-")
            {
                return Err("prod profile rejects development MARKHAND_AUTH_KID values".into());
            }
            if self.auth.access_token_ttl_secs > PROD_MAX_ACCESS_TOKEN_TTL_SECS {
                return Err(format!(
                    "prod MARKHAND_AUTH_ACCESS_TOKEN_TTL_SECS must be <= {PROD_MAX_ACCESS_TOKEN_TTL_SECS}"
                ));
            }
            if self.auth.argon2.memory_kib < PROD_MIN_ARGON2_MEMORY_KIB
                || self.auth.argon2.time_cost < PROD_MIN_ARGON2_TIME_COST
            {
                return Err(format!(
                    "prod Argon2id requires memory_kib>={PROD_MIN_ARGON2_MEMORY_KIB} and time_cost>={PROD_MIN_ARGON2_TIME_COST}"
                ));
            }
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
        if self.minio_operation_timeout_secs == 0
            || self.minio_operation_timeout_secs > MAX_MINIO_OPERATION_TIMEOUT_SECS
        {
            return Err(format!(
                "MARKHAND_MINIO_OPERATION_TIMEOUT_SECS must be between 1 and {MAX_MINIO_OPERATION_TIMEOUT_SECS}"
            ));
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

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeEndpoints {
    pub database_url: SecretString,
    pub qdrant_url: String,
    pub minio_url: String,
}

impl fmt::Debug for RuntimeEndpoints {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeEndpoints")
            .field("database_url", &"[REDACTED]")
            .field("qdrant_url", &"[REDACTED_ENDPOINT]")
            .field("minio_url", &"[REDACTED_ENDPOINT]")
            .finish()
    }
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

fn u32_value(
    file: Option<&ConfigFile>,
    env: &BTreeMap<String, String>,
    env_name: &str,
    from_file: impl FnOnce(&ConfigFile) -> Option<u64>,
    default: u32,
) -> Result<u32, String> {
    let value = numeric_value(file, env, env_name, from_file, u64::from(default))?;
    u32::try_from(value).map_err(|_| format!("{env_name} is out of range for u32"))
}

fn usize_value(
    file: Option<&ConfigFile>,
    env: &BTreeMap<String, String>,
    env_name: &str,
    from_file: impl FnOnce(&ConfigFile) -> Option<u64>,
    default: usize,
) -> Result<usize, String> {
    let value = numeric_value(file, env, env_name, from_file, default as u64)?;
    usize::try_from(value).map_err(|_| format!("{env_name} is out of range for usize"))
}

fn bool_value(
    file: Option<&ConfigFile>,
    env: &BTreeMap<String, String>,
    env_name: &str,
    from_file: impl FnOnce(&ConfigFile) -> Option<bool>,
    default: bool,
) -> Result<bool, String> {
    match env.get(env_name) {
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!("{env_name} must be a boolean")),
        },
        None => Ok(file.and_then(from_file).unwrap_or(default)),
    }
}

fn required_url(value: Option<&str>, name: &str) -> Result<String, String> {
    let value = value.ok_or_else(|| format!("server requires {name}"))?;
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.contains('?') || trimmed.contains('#') {
        return Err(format!(
            "{name} must not include query parameters or fragments"
        ));
    }
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|_| format!("{name} must be an absolute URL"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("{name} must use http or https"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("{name} must not embed userinfo credentials"));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!(
            "{name} must not include query parameters or fragments"
        ));
    }
    if parsed.host_str().is_none() {
        return Err(format!("{name} must include a host"));
    }
    Ok(trimmed.to_string())
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

fn load_file(path: &Path, role: RuntimeRole) -> Result<ConfigFile, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|_| "cannot read MARKHAND_CONFIG_FILE".to_string())?;
    let mut value: serde_json::Value = serde_json::from_str(&source)
        .map_err(|_| "MARKHAND_CONFIG_FILE contains invalid JSON".to_string())?;
    if role == RuntimeRole::Worker {
        let object = value
            .as_object_mut()
            .ok_or_else(|| "MARKHAND_CONFIG_FILE contains invalid JSON".to_string())?;
        if [
            "authIssuer",
            "authAudience",
            "authSigningKey",
            "authKid",
            "authAlg",
        ]
        .iter()
        .any(|key| object.contains_key(*key))
        {
            return Err("worker configuration must not contain API authentication settings".into());
        }
    }
    serde_json::from_value(value).map_err(|_| "MARKHAND_CONFIG_FILE contains invalid JSON".into())
}

fn reject_worker_auth_environment(env: &BTreeMap<String, String>) -> Result<(), String> {
    if env.keys().any(|key| {
        matches!(
            key.as_str(),
            "MARKHAND_AUTH_ISSUER"
                | "MARKHAND_AUTH_AUDIENCE"
                | "MARKHAND_AUTH_SIGNING_KEY"
                | "MARKHAND_AUTH_ALG"
                | "MARKHAND_AUTH_KID"
                | "MARKHAND_AUTH_ACCESS_TOKEN_TTL_SECS"
                | "MARKHAND_AUTH_REFRESH_TOKEN_TTL_SECS"
                | "MARKHAND_AUTH_ARGON2_MEMORY_KIB"
                | "MARKHAND_AUTH_ARGON2_TIME_COST"
                | "MARKHAND_AUTH_ARGON2_PARALLELISM"
        )
    }) {
        Err("worker environment must not contain API authentication settings".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigFile, Profile, RuntimeRole, ServerConfig};
    use std::collections::BTreeMap;

    #[test]
    fn environment_overrides_file_and_defaults() {
        let file = ConfigFile {
            profile: Some("test".into()),
            bind_addr: Some("127.0.0.1:9000".into()),
            database_url: Some("postgres://file-secret".into()),
            qdrant_url: Some("http://qdrant.test".into()),
            qdrant_api_key: None,
            minio_url: Some("http://minio.test".into()),
            minio_access_key: None,
            minio_secret_key: None,
            minio_bucket: None,
            minio_region: None,
            minio_path_style: None,
            minio_operation_timeout_secs: None,
            auth_issuer: None,
            auth_audience: None,
            auth_signing_key: None,
            auth_alg: None,
            auth_kid: None,
            auth_access_token_ttl_secs: None,
            auth_refresh_token_ttl_secs: None,
            auth_argon2_memory_kib: None,
            auth_argon2_time_cost: None,
            auth_argon2_parallelism: None,
            max_upload_bytes: None,
            job_lease_seconds: None,
            max_archive_entries: None,
            max_archive_uncompressed_bytes: None,
            max_archive_compression_ratio: None,
            max_pdf_pages: None,
            max_image_pixels: None,
            max_audio_duration_secs: None,
            max_multipart_parts: None,
            max_part_header_bytes: None,
            upload_timeout_secs: None,
            upload_idle_timeout_secs: None,
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
            ("MARKHAND_AUTH_KID".into(), "prod-key-1".into()),
            (
                "MARKHAND_DATABASE_URL".into(),
                "postgres://postgres:postgres@localhost/db".into(),
            ),
            (
                "MARKHAND_QDRANT_URL".into(),
                "https://qdrant.example".into(),
            ),
            ("MARKHAND_MINIO_URL".into(), "https://minio.example".into()),
            (
                "MARKHAND_INDEX_SIGNATURE".into(),
                "d54db7b6de20b51a416670927eeab346256c9b891732965e51586fac333c1835".into(),
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
            ("MARKHAND_AUTH_KID".into(), "prod-key-1".into()),
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
    fn auth_requires_kid_and_rejects_dev_kid_in_prod() {
        let mut env = BTreeMap::from([
            ("MARKHAND_PROFILE".into(), "dev".into()),
            (
                "MARKHAND_AUTH_ISSUER".into(),
                "https://issuer.example.test".into(),
            ),
            ("MARKHAND_AUTH_AUDIENCE".into(), "markhand-api".into()),
            (
                "MARKHAND_AUTH_SIGNING_KEY".into(),
                "this-test-signing-key-is-at-least-32-bytes".into(),
            ),
        ]);
        assert_eq!(
            ServerConfig::from_sources(None, &env).unwrap_err(),
            "MARKHAND_AUTH_KID is required when issuer is set"
        );
        env.insert("MARKHAND_AUTH_KID".into(), "dev-local".into());
        let config = ServerConfig::from_sources(None, &env).unwrap();
        assert_eq!(config.auth.kid.as_deref(), Some("dev-local"));
        assert!(!format!("{config:?}").contains("this-test-signing-key"));

        env.insert("MARKHAND_PROFILE".into(), "prod".into());
        env.insert("MARKHAND_BIND_ADDR".into(), "10.0.0.10:8787".into());
        env.insert(
            "MARKHAND_DATABASE_URL".into(),
            "postgres://app:secret@postgres.internal/markhand?sslmode=require".into(),
        );
        env.insert(
            "MARKHAND_QDRANT_URL".into(),
            "https://qdrant.internal".into(),
        );
        env.insert("MARKHAND_MINIO_URL".into(), "https://minio.internal".into());
        env.insert(
            "MARKHAND_INDEX_SIGNATURE".into(),
            "d54db7b6de20b51a416670927eeab346256c9b891732965e51586fac333c1835".into(),
        );
        assert!(ServerConfig::from_sources(None, &env)
            .unwrap_err()
            .contains("development MARKHAND_AUTH_KID"));
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

        let orphan_audience =
            BTreeMap::from([("MARKHAND_AUTH_AUDIENCE".into(), "markhand-api".into())]);
        assert_eq!(
            ServerConfig::from_sources(None, &orphan_audience).unwrap_err(),
            "MARKHAND_AUTH_ISSUER is required when authentication is configured"
        );

        let invalid_signature =
            BTreeMap::from([("MARKHAND_INDEX_SIGNATURE".into(), "not-a-signature".into())]);
        assert_eq!(
            ServerConfig::from_sources(None, &invalid_signature).unwrap_err(),
            "MARKHAND_INDEX_SIGNATURE must be a 64-character hex digest"
        );
    }

    #[test]
    fn storage_config_requires_minio_credentials_and_redacts_secrets() {
        let env = BTreeMap::from([
            (
                "MARKHAND_DATABASE_URL".into(),
                "postgres://markhand@localhost/markhand".into(),
            ),
            ("MARKHAND_QDRANT_URL".into(), "http://qdrant.test".into()),
            ("MARKHAND_MINIO_URL".into(), "http://minio.test".into()),
        ]);
        let config = ServerConfig::from_sources(None, &env).unwrap();
        assert_eq!(
            config.storage_config().unwrap_err(),
            "server requires MARKHAND_MINIO_ACCESS_KEY"
        );

        let env = BTreeMap::from([
            (
                "MARKHAND_DATABASE_URL".into(),
                "postgres://markhand@localhost/markhand".into(),
            ),
            ("MARKHAND_QDRANT_URL".into(), "http://qdrant.test".into()),
            ("MARKHAND_MINIO_URL".into(), "http://minio.test".into()),
            ("MARKHAND_MINIO_ACCESS_KEY".into(), "access-secret".into()),
            ("MARKHAND_MINIO_SECRET_KEY".into(), "secret-secret".into()),
            ("MARKHAND_MINIO_BUCKET".into(), "markhand-documents".into()),
        ]);
        let config = ServerConfig::from_sources(None, &env).unwrap();
        let storage = config.storage_config().unwrap();
        let debug = format!("{storage:?}");
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("secret-secret"));
        assert!(debug.contains("[REDACTED]"));
        assert!(storage.minio().path_style());
        assert!(!format!("{storage:?}").contains("qdrant.test"));
        assert!(!format!("{storage:?}").contains("minio.test"));
    }

    #[test]
    fn endpoint_urls_reject_embedded_credentials() {
        let env = BTreeMap::from([
            (
                "MARKHAND_DATABASE_URL".into(),
                "postgres://markhand@localhost/markhand".into(),
            ),
            (
                "MARKHAND_QDRANT_URL".into(),
                "http://user:pass@qdrant.test".into(),
            ),
            ("MARKHAND_MINIO_URL".into(), "http://minio.test".into()),
            ("MARKHAND_MINIO_ACCESS_KEY".into(), "access-secret".into()),
            ("MARKHAND_MINIO_SECRET_KEY".into(), "secret-secret".into()),
        ]);
        let config = ServerConfig::from_sources(None, &env).unwrap();
        assert!(config.runtime_endpoints().unwrap_err().contains("userinfo"));
    }

    #[test]
    fn endpoint_urls_reject_any_query_or_fragment() {
        for bad in [
            "http://qdrant.test?sig=abc",
            "http://qdrant.test#frag",
            "http://qdrant.test/?x=1",
            "http://minio.test?token=x",
        ] {
            let env = BTreeMap::from([
                (
                    "MARKHAND_DATABASE_URL".into(),
                    "postgres://markhand@localhost/markhand".into(),
                ),
                ("MARKHAND_QDRANT_URL".into(), bad.into()),
                ("MARKHAND_MINIO_URL".into(), "http://minio.test".into()),
            ]);
            let config = ServerConfig::from_sources(None, &env).unwrap();
            let err = config.runtime_endpoints().unwrap_err();
            assert!(
                err.contains("query") || err.contains("fragment") || err.contains("userinfo"),
                "expected rejection for {bad}, got {err}"
            );
        }
    }

    #[test]
    fn server_config_debug_redacts_endpoints_and_secrets() {
        let env = BTreeMap::from([
            (
                "MARKHAND_DATABASE_URL".into(),
                "postgres://markhand:top-secret@db.internal/markhand".into(),
            ),
            (
                "MARKHAND_QDRANT_URL".into(),
                "http://qdrant.internal:6333".into(),
            ),
            (
                "MARKHAND_MINIO_URL".into(),
                "http://minio.internal:9000".into(),
            ),
            ("MARKHAND_MINIO_ACCESS_KEY".into(), "access-secret".into()),
            ("MARKHAND_MINIO_SECRET_KEY".into(), "secret-secret".into()),
            ("MARKHAND_QDRANT_API_KEY".into(), "qdrant-secret-key".into()),
        ]);
        let config = ServerConfig::from_sources(None, &env).unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("top-secret"));
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("secret-secret"));
        assert!(!debug.contains("qdrant-secret"));
        assert!(!debug.contains("qdrant.internal"));
        assert!(!debug.contains("minio.internal"));
        assert!(!debug.contains("db.internal"));
        assert!(debug.contains("[REDACTED"));
        let endpoints = config.runtime_endpoints().unwrap();
        let endpoints_debug = format!("{endpoints:?}");
        assert!(!endpoints_debug.contains("qdrant.internal"));
        assert!(!endpoints_debug.contains("minio.internal"));
        assert!(!endpoints_debug.contains("top-secret"));
    }

    #[test]
    fn worker_configuration_rejects_api_authentication() {
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
            ("MARKHAND_MINIO_URL".into(), "https://minio.internal".into()),
            (
                "MARKHAND_AUTH_SIGNING_KEY".into(),
                "must-not-be-loaded-by-a-converter-worker".into(),
            ),
            (
                "MARKHAND_INDEX_SIGNATURE".into(),
                "d54db7b6de20b51a416670927eeab346256c9b891732965e51586fac333c1835".into(),
            ),
        ]);
        assert_eq!(
            ServerConfig::from_sources_for_role(None, &env, RuntimeRole::Worker).unwrap_err(),
            "worker environment must not contain API authentication settings"
        );
    }
}
