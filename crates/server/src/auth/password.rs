//! Argon2id password hashing with configurable parameters and rehash-on-login.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use thiserror::Error;

use crate::config::{Argon2Config, SecretString};

/// Errors from password hashing or verification.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PasswordError {
    #[error("password hashing failed")]
    Hash,
    #[error("password verification failed")]
    Verify,
    #[error("stored password hash is invalid")]
    InvalidHash,
}

/// Hashes a password with Argon2id using the configured parameters.
pub fn hash_password(password: &str, params: &Argon2Config) -> Result<SecretString, PasswordError> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let argon = build_argon2(params)?;
    let encoded = argon
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| PasswordError::Hash)?
        .to_string();
    Ok(SecretString::new(encoded))
}

/// Verifies `password` against a stored PHC hash string.
pub fn verify_password(password: &str, password_hash: &str) -> Result<(), PasswordError> {
    let parsed = PasswordHash::new(password_hash).map_err(|_| PasswordError::InvalidHash)?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .map_err(|_| PasswordError::Verify)
}

/// Returns true when the stored hash does not match the configured Argon2id parameters.
pub fn needs_rehash(password_hash: &str, params: &Argon2Config) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(password_hash).map_err(|_| PasswordError::InvalidHash)?;
    let stored = Params::try_from(&parsed).map_err(|_| PasswordError::InvalidHash)?;
    let algorithm =
        Algorithm::try_from(parsed.algorithm).map_err(|_| PasswordError::InvalidHash)?;
    Ok(algorithm != Algorithm::Argon2id
        || stored.m_cost() != params.memory_kib
        || stored.t_cost() != params.time_cost
        || stored.p_cost() != params.parallelism)
}

fn build_argon2(params: &Argon2Config) -> Result<Argon2<'static>, PasswordError> {
    let argon_params = Params::new(
        params.memory_kib,
        params.time_cost,
        params.parallelism,
        None,
    )
    .map_err(|_| PasswordError::Hash)?;
    Ok(Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        argon_params,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Argon2Config;

    fn fast_params() -> Argon2Config {
        Argon2Config {
            memory_kib: 8_192,
            time_cost: 1,
            parallelism: 1,
        }
    }

    #[test]
    fn hash_verifies_and_wrong_password_fails() {
        let params = fast_params();
        let hash = hash_password("correct horse battery", &params).unwrap();
        assert!(verify_password("correct horse battery", hash.expose()).is_ok());
        assert_eq!(
            verify_password("wrong-password", hash.expose()),
            Err(PasswordError::Verify)
        );
        assert!(!format!("{hash:?}").contains("correct"));
    }

    #[test]
    fn rehash_triggered_when_params_change() {
        let weak = fast_params();
        let hash = hash_password("secret-value", &weak).unwrap();
        assert!(!needs_rehash(hash.expose(), &weak).unwrap());
        let stronger = Argon2Config {
            memory_kib: 16_384,
            time_cost: 2,
            parallelism: 1,
        };
        assert!(needs_rehash(hash.expose(), &stronger).unwrap());
    }
}
