//! Signed JWT access tokens with pinned algorithm, issuer, audience, and kid.

use std::fmt;

use base64::Engine;
use chrono::{Duration, Utc};
use jsonwebtoken::{
    decode, decode_header, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::config::{AuthConfig, JwtAlgorithm, SecretString};

/// Access-token claims. `org_id` / `sid` are hints only — authorization loads from PG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessClaims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
    /// Org id hint (authorization must re-resolve from PostgreSQL).
    pub org_id: String,
    /// Session / refresh-token family id.
    pub sid: String,
}

/// Errors from JWT signing or verification.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum JwtError {
    #[error("authentication is not configured")]
    NotConfigured,
    #[error("token encoding failed")]
    Encode,
    #[error("token is invalid")]
    Invalid,
    #[error("token algorithm is not allowed")]
    Algorithm,
    #[error("token kid mismatch")]
    Kid,
    #[error("token claims are invalid")]
    Claims,
}

/// HS256 signer/verifier bound to pinned AuthConfig values.
#[derive(Clone)]
pub struct JwtKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
    issuer: String,
    audience: String,
    kid: String,
    access_token_ttl_secs: u64,
}

impl fmt::Debug for JwtKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtKeys")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("kid", &self.kid)
            .field("access_token_ttl_secs", &self.access_token_ttl_secs)
            .field("encoding", &"[REDACTED]")
            .field("decoding", &"[REDACTED]")
            .finish()
    }
}

impl JwtKeys {
    /// Builds keys from a fully configured [`AuthConfig`].
    ///
    /// Defense-in-depth: independently enforces HS256 key length ≥ 32 bytes and
    /// access TTL in `(0, 900]` even if callers bypass [`ServerConfig`] validation.
    pub fn from_auth(auth: &AuthConfig) -> Result<Self, JwtError> {
        let issuer = auth.issuer.as_deref().ok_or(JwtError::NotConfigured)?;
        let audience = auth.audience.as_deref().ok_or(JwtError::NotConfigured)?;
        let signing_key = auth.signing_key.as_ref().ok_or(JwtError::NotConfigured)?;
        let kid = auth.kid.as_deref().ok_or(JwtError::NotConfigured)?;
        if auth.alg != JwtAlgorithm::Hs256 {
            return Err(JwtError::Algorithm);
        }
        let secret = signing_key.expose().as_bytes();
        if secret.len() < 32 {
            return Err(JwtError::Claims);
        }
        if auth.access_token_ttl_secs == 0 || auth.access_token_ttl_secs > 900 {
            return Err(JwtError::Claims);
        }
        if kid.trim().is_empty() || issuer.is_empty() || audience.is_empty() {
            return Err(JwtError::NotConfigured);
        }
        Ok(Self {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
            issuer: issuer.to_string(),
            audience: audience.to_string(),
            kid: kid.to_string(),
            access_token_ttl_secs: auth.access_token_ttl_secs,
        })
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    pub fn audience(&self) -> &str {
        &self.audience
    }

    /// Signs a short-lived access token for `user_id` / `org_id` / session family `sid`.
    pub fn sign_access_token(
        &self,
        user_id: Uuid,
        org_id: Uuid,
        sid: Uuid,
    ) -> Result<SecretString, JwtError> {
        let now = Utc::now();
        let ttl = Duration::seconds(self.access_token_ttl_secs as i64);
        let claims = AccessClaims {
            sub: user_id.to_string(),
            iss: self.issuer.clone(),
            aud: self.audience.clone(),
            iat: now.timestamp(),
            nbf: now.timestamp(),
            exp: (now + ttl).timestamp(),
            org_id: org_id.to_string(),
            sid: sid.to_string(),
        };
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(self.kid.clone());
        let token = encode(&header, &claims, &self.encoding).map_err(|_| JwtError::Encode)?;
        Ok(SecretString::new(token))
    }

    /// Verifies signature and pinned alg/iss/aud/kid/exp/nbf. Rejects `none` and wrong alg.
    pub fn verify_access_token(&self, token: &str) -> Result<AccessClaims, JwtError> {
        // Reject `alg: none` even if the library cannot parse it into Algorithm.
        let header_json = token
            .split('.')
            .next()
            .ok_or(JwtError::Invalid)
            .and_then(|part| {
                Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, part)
                    .or_else(|_| Engine::decode(&base64::engine::general_purpose::URL_SAFE, part))
                    .map_err(|_| JwtError::Invalid)
            })?;
        let header_value: serde_json::Value =
            serde_json::from_slice(&header_json).map_err(|_| JwtError::Invalid)?;
        match header_value.get("alg").and_then(|value| value.as_str()) {
            Some("HS256") => {}
            Some("none") | Some("None") | Some("NONE") => return Err(JwtError::Algorithm),
            Some(_) => return Err(JwtError::Algorithm),
            None => return Err(JwtError::Algorithm),
        }

        let header = decode_header(token).map_err(|_| JwtError::Invalid)?;
        if header.alg != Algorithm::HS256 {
            return Err(JwtError::Algorithm);
        }
        if header.kid.as_deref() != Some(self.kid.as_str()) {
            return Err(JwtError::Kid);
        }

        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_nbf = true;
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "iat", "nbf", "sub", "iss", "aud"]);

        let data = decode::<AccessClaims>(token, &self.decoding, &validation)
            .map_err(|_| JwtError::Invalid)?;
        if data.header.alg != Algorithm::HS256 {
            return Err(JwtError::Algorithm);
        }
        if data.header.kid.as_deref() != Some(self.kid.as_str()) {
            return Err(JwtError::Kid);
        }
        let claims = data.claims;
        if claims.sub.is_empty()
            || Uuid::parse_str(&claims.sub).is_err()
            || Uuid::parse_str(&claims.org_id).is_err()
            || Uuid::parse_str(&claims.sid).is_err()
        {
            return Err(JwtError::Claims);
        }
        // jsonwebtoken 10.x does not enforce iat; do it explicitly with checked math.
        self.validate_iat_lifetime(&claims)?;
        Ok(claims)
    }

    fn validate_iat_lifetime(&self, claims: &AccessClaims) -> Result<(), JwtError> {
        const LEEWAY_SECS: i64 = 60;
        let now = Utc::now().timestamp();
        // Reject future iat (beyond small clock skew leeway).
        let max_iat = now.checked_add(LEEWAY_SECS).ok_or(JwtError::Claims)?;
        if claims.iat > max_iat {
            return Err(JwtError::Claims);
        }
        // exp must be strictly after iat.
        if claims.exp <= claims.iat {
            return Err(JwtError::Claims);
        }
        let lifetime = claims.exp.checked_sub(claims.iat).ok_or(JwtError::Claims)?;
        let max_ttl = i64::try_from(self.access_token_ttl_secs).map_err(|_| JwtError::Claims)?;
        if lifetime > max_ttl {
            return Err(JwtError::Claims);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Argon2Config, AuthConfig, JwtAlgorithm, SecretString};
    use base64::Engine;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

    fn test_auth() -> AuthConfig {
        AuthConfig {
            issuer: Some("https://issuer.markhand.test".into()),
            audience: Some("markhand-api".into()),
            signing_key: Some(SecretString::new("unit-test-signing-key-at-least-32b!")),
            alg: JwtAlgorithm::Hs256,
            kid: Some("test-kid-1".into()),
            access_token_ttl_secs: 900,
            refresh_token_ttl_secs: 3600,
            argon2: Argon2Config {
                memory_kib: 8_192,
                time_cost: 1,
                parallelism: 1,
            },
        }
    }

    fn keys() -> JwtKeys {
        JwtKeys::from_auth(&test_auth()).unwrap()
    }

    #[test]
    fn sign_verify_round_trip_and_debug_redacts_secret() {
        let keys = keys();
        let user = Uuid::new_v4();
        let org = Uuid::new_v4();
        let sid = Uuid::new_v4();
        let token = keys.sign_access_token(user, org, sid).unwrap();
        let claims = keys.verify_access_token(token.expose()).unwrap();
        assert_eq!(claims.sub, user.to_string());
        assert_eq!(claims.org_id, org.to_string());
        assert_eq!(claims.sid, sid.to_string());
        assert!(!format!("{keys:?}").contains("unit-test-signing-key"));
        assert!(!format!("{token:?}").contains(token.expose()));
    }

    #[test]
    fn rejects_wrong_issuer_audience_kid_and_tamper() {
        let keys = keys();
        let token = keys
            .sign_access_token(Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
            .unwrap();

        let mut other = test_auth();
        other.issuer = Some("https://other.example".into());
        assert_eq!(
            JwtKeys::from_auth(&other)
                .unwrap()
                .verify_access_token(token.expose()),
            Err(JwtError::Invalid)
        );

        let mut other = test_auth();
        other.audience = Some("other-aud".into());
        assert_eq!(
            JwtKeys::from_auth(&other)
                .unwrap()
                .verify_access_token(token.expose()),
            Err(JwtError::Invalid)
        );

        let mut other = test_auth();
        other.kid = Some("other-kid".into());
        assert_eq!(
            JwtKeys::from_auth(&other)
                .unwrap()
                .verify_access_token(token.expose()),
            Err(JwtError::Kid)
        );

        let mut tampered = token.expose().to_string();
        tampered.push('x');
        assert_eq!(keys.verify_access_token(&tampered), Err(JwtError::Invalid));
    }

    #[test]
    fn rejects_none_and_wrong_algorithm() {
        let keys = keys();
        let user = Uuid::new_v4();
        let claims = AccessClaims {
            sub: user.to_string(),
            iss: keys.issuer.clone(),
            aud: keys.audience.clone(),
            iat: Utc::now().timestamp(),
            nbf: Utc::now().timestamp(),
            exp: (Utc::now() + Duration::minutes(5)).timestamp(),
            org_id: Uuid::new_v4().to_string(),
            sid: Uuid::new_v4().to_string(),
        };

        // Manually craft an unsigned "none" token.
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&claims).unwrap());
        let none_token = format!("{header}.{payload}.");
        assert_eq!(
            keys.verify_access_token(&none_token),
            Err(JwtError::Algorithm)
        );

        let mut header = Header::new(Algorithm::HS384);
        header.kid = Some(keys.kid.clone());
        let wrong_alg = encode(
            &header,
            &claims,
            &EncodingKey::from_secret(b"unit-test-signing-key-at-least-32b!"),
        )
        .unwrap();
        assert_eq!(
            keys.verify_access_token(&wrong_alg),
            Err(JwtError::Algorithm)
        );
    }

    #[test]
    fn rejects_expired_and_not_before() {
        let keys = keys();
        let now = Utc::now();
        let claims = AccessClaims {
            sub: Uuid::new_v4().to_string(),
            iss: keys.issuer.clone(),
            aud: keys.audience.clone(),
            iat: (now - Duration::hours(2)).timestamp(),
            nbf: (now - Duration::hours(2)).timestamp(),
            exp: (now - Duration::hours(1)).timestamp(),
            org_id: Uuid::new_v4().to_string(),
            sid: Uuid::new_v4().to_string(),
        };
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(keys.kid.clone());
        let expired = encode(&header, &claims, &keys.encoding).unwrap();
        assert_eq!(keys.verify_access_token(&expired), Err(JwtError::Invalid));

        let future = AccessClaims {
            nbf: (now + Duration::hours(1)).timestamp(),
            exp: (now + Duration::hours(2)).timestamp(),
            iat: now.timestamp(),
            ..claims
        };
        let not_before = encode(&header, &future, &keys.encoding).unwrap();
        assert_eq!(
            keys.verify_access_token(&not_before),
            Err(JwtError::Invalid)
        );
    }

    #[test]
    fn rejects_overlong_lifetime_future_iat_and_exp_before_iat() {
        let keys = keys();
        let now = Utc::now();
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(keys.kid.clone());
        let base = AccessClaims {
            sub: Uuid::new_v4().to_string(),
            iss: keys.issuer.clone(),
            aud: keys.audience.clone(),
            iat: now.timestamp(),
            nbf: now.timestamp(),
            exp: (now + Duration::seconds(901)).timestamp(),
            org_id: Uuid::new_v4().to_string(),
            sid: Uuid::new_v4().to_string(),
        };
        let overlong = encode(&header, &base, &keys.encoding).unwrap();
        assert_eq!(
            keys.verify_access_token(&overlong),
            Err(JwtError::Claims),
            "lifetime > configured access TTL must be rejected"
        );

        let future_iat = AccessClaims {
            iat: (now + Duration::hours(2)).timestamp(),
            nbf: now.timestamp(),
            exp: (now + Duration::hours(2) + Duration::minutes(5)).timestamp(),
            ..base.clone()
        };
        let future = encode(&header, &future_iat, &keys.encoding).unwrap();
        assert_eq!(
            keys.verify_access_token(&future),
            Err(JwtError::Claims),
            "future iat must be rejected"
        );

        let inverted = AccessClaims {
            iat: (now + Duration::seconds(30)).timestamp(),
            nbf: now.timestamp(),
            // Still in the future for the library clock check, but earlier than iat.
            exp: (now + Duration::seconds(20)).timestamp(),
            ..base
        };
        let bad_exp = encode(&header, &inverted, &keys.encoding).unwrap();
        assert_eq!(
            keys.verify_access_token(&bad_exp),
            Err(JwtError::Claims),
            "exp <= iat must be rejected"
        );
    }

    #[test]
    fn from_auth_rejects_short_key_and_overlong_ttl() {
        let mut auth = test_auth();
        auth.signing_key = Some(SecretString::new("too-short"));
        assert!(matches!(JwtKeys::from_auth(&auth), Err(JwtError::Claims)));

        let mut auth = test_auth();
        auth.access_token_ttl_secs = 901;
        assert!(matches!(JwtKeys::from_auth(&auth), Err(JwtError::Claims)));

        let mut auth = test_auth();
        auth.access_token_ttl_secs = 0;
        assert!(matches!(JwtKeys::from_auth(&auth), Err(JwtError::Claims)));
    }
}
