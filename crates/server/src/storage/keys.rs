//! Opaque, traversal-safe object-key builder (ADR 0007).
//!
//! Keys contain only a namespace plus hashed org/version identity and a
//! server-generated object id. User filenames and raw user input never appear
//! in the path; original names belong in object metadata only.
//!
//! Parsing and authorization are org-bound: a key is only usable when its
//! org-opaque segment matches the authorized org (recomputed and compared).

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::error::StorageError;

const KEY_DOMAIN: &[u8] = b"markhand-object-key-v1";
/// Full SHA-256 hex length for opaque identity segments (256 bits).
const OPAQUE_HEX_LEN: usize = 64;
/// Hex length of a UUID without hyphens.
const OBJECT_ID_HEX_LEN: usize = 32;

/// Object-key namespace (quarantine vs post-conversion trusted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectNamespace {
    Quarantine,
    Trusted,
}

impl ObjectNamespace {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quarantine => "quarantine",
            Self::Trusted => "trusted",
        }
    }

    fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "quarantine" => Ok(Self::Quarantine),
            "trusted" => Ok(Self::Trusted),
            _ => Err(StorageError::InvalidKey),
        }
    }
}

/// Parsed, validated object key (round-trips with [`ObjectKey::as_str`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectKey {
    namespace: ObjectNamespace,
    org_opaque: String,
    version_opaque: Option<String>,
    object_id: String,
}

impl ObjectKey {
    pub const fn namespace(&self) -> ObjectNamespace {
        self.namespace
    }

    pub fn org_opaque(&self) -> &str {
        &self.org_opaque
    }

    pub fn version_opaque(&self) -> Option<&str> {
        self.version_opaque.as_deref()
    }

    pub fn object_id(&self) -> &str {
        &self.object_id
    }

    /// Canonical key string (`namespace/...` only; never a filename).
    pub fn as_str(&self) -> String {
        match (&self.namespace, &self.version_opaque) {
            (ObjectNamespace::Quarantine, None) => {
                format!(
                    "{}/{}/{}",
                    self.namespace.as_str(),
                    self.org_opaque,
                    self.object_id
                )
            }
            (ObjectNamespace::Trusted, Some(version)) => {
                format!(
                    "{}/{}/{}/{}",
                    self.namespace.as_str(),
                    self.org_opaque,
                    version,
                    self.object_id
                )
            }
            _ => unreachable!("object key invariants enforce namespace/version pairing"),
        }
    }

    /// True when this key's org-opaque segment matches `org_id`.
    pub fn belongs_to_org(&self, org_id: Uuid) -> bool {
        !org_id.is_nil() && self.org_opaque == opaque_identity("org", org_id)
    }

    /// True when a trusted key's version-opaque matches `version_id`.
    pub fn belongs_to_version(&self, version_id: Uuid) -> bool {
        match &self.version_opaque {
            Some(opaque) => {
                !version_id.is_nil() && opaque.as_str() == opaque_identity("version", version_id)
            }
            None => false,
        }
    }
}

/// Build a quarantine key: `quarantine/{org_opaque}/{object_id}`.
///
/// Rejects nil identities. `original_filename` is unused in the key path.
pub fn quarantine_key(
    org_id: Uuid,
    object_id: Uuid,
    _original_filename: Option<&str>,
) -> Result<ObjectKey, StorageError> {
    reject_nil_ids(&[org_id, object_id])?;
    Ok(ObjectKey {
        namespace: ObjectNamespace::Quarantine,
        org_opaque: opaque_identity("org", org_id),
        version_opaque: None,
        object_id: object_id_hex(object_id),
    })
}

/// Build a trusted (post-conversion) key: `trusted/{org_opaque}/{version_opaque}/{object_id}`.
pub fn trusted_key(
    org_id: Uuid,
    version_id: Uuid,
    object_id: Uuid,
    _original_filename: Option<&str>,
) -> Result<ObjectKey, StorageError> {
    reject_nil_ids(&[org_id, version_id, object_id])?;
    Ok(ObjectKey {
        namespace: ObjectNamespace::Trusted,
        org_opaque: opaque_identity("org", org_id),
        version_opaque: Some(opaque_identity("version", version_id)),
        object_id: object_id_hex(object_id),
    })
}

/// Parse a key and authorize it for `org_id` (fail closed on org mismatch).
///
/// There is no public unbound parse that yields a usable cross-org key.
pub fn parse_key_for_org(raw: &str, org_id: Uuid) -> Result<ObjectKey, StorageError> {
    if org_id.is_nil() {
        return Err(StorageError::MissingScope);
    }
    let key = parse_key_structure(raw)?;
    authorize_key_for_org(&key, org_id)?;
    Ok(key)
}

/// Verify an already-built key belongs to `org_id`.
pub fn authorize_key_for_org(key: &ObjectKey, org_id: Uuid) -> Result<(), StorageError> {
    if org_id.is_nil() {
        return Err(StorageError::MissingScope);
    }
    if !key.belongs_to_org(org_id) {
        return Err(StorageError::KeyOrgMismatch);
    }
    Ok(())
}

/// Verify a trusted key's version segment matches `version_id`.
pub fn authorize_key_for_version(key: &ObjectKey, version_id: Uuid) -> Result<(), StorageError> {
    if version_id.is_nil() {
        return Err(StorageError::MissingScope);
    }
    match key.namespace() {
        ObjectNamespace::Trusted => {
            if key.belongs_to_version(version_id) {
                Ok(())
            } else {
                Err(StorageError::KeyOrgMismatch)
            }
        }
        ObjectNamespace::Quarantine => Ok(()),
    }
}

/// Structural parse only (crate-private). Callers must authorize via org.
pub(crate) fn parse_key_structure(raw: &str) -> Result<ObjectKey, StorageError> {
    reject_malformed_input(raw)?;
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.iter().any(|part| part.is_empty()) {
        return Err(StorageError::InvalidKey);
    }
    let namespace = ObjectNamespace::parse(parts[0])?;
    match namespace {
        ObjectNamespace::Quarantine => {
            if parts.len() != 3 {
                return Err(StorageError::InvalidKey);
            }
            Ok(ObjectKey {
                namespace,
                org_opaque: require_hex(parts[1], OPAQUE_HEX_LEN)?,
                version_opaque: None,
                object_id: require_hex(parts[2], OBJECT_ID_HEX_LEN)?,
            })
        }
        ObjectNamespace::Trusted => {
            if parts.len() != 4 {
                return Err(StorageError::InvalidKey);
            }
            Ok(ObjectKey {
                namespace,
                org_opaque: require_hex(parts[1], OPAQUE_HEX_LEN)?,
                version_opaque: Some(require_hex(parts[2], OPAQUE_HEX_LEN)?),
                object_id: require_hex(parts[3], OBJECT_ID_HEX_LEN)?,
            })
        }
    }
}

pub(crate) fn opaque_identity(kind: &str, id: Uuid) -> String {
    let mut hasher = Sha256::new();
    hasher.update(KEY_DOMAIN);
    hasher.update(kind.as_bytes());
    hasher.update(id.as_bytes());
    hex::encode(hasher.finalize())
}

fn object_id_hex(id: Uuid) -> String {
    hex::encode(id.as_bytes())
}

fn reject_nil_ids(ids: &[Uuid]) -> Result<(), StorageError> {
    if ids.iter().any(|id| id.is_nil()) {
        Err(StorageError::MissingScope)
    } else {
        Ok(())
    }
}

fn require_hex(value: &str, expected_len: usize) -> Result<String, StorageError> {
    if value.len() != expected_len || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StorageError::InvalidKey);
    }
    if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(StorageError::InvalidKey);
    }
    Ok(value.to_string())
}

fn reject_malformed_input(raw: &str) -> Result<(), StorageError> {
    if raw.is_empty()
        || raw.starts_with('/')
        || raw.contains('\\')
        || raw.contains('\0')
        || raw.chars().any(|ch| ch.is_control())
        || raw
            .split('/')
            .any(|part| part == ".." || part == "." || part.contains("..") || part.contains('\\'))
    {
        return Err(StorageError::InvalidKey);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_and_trusted_round_trip() {
        let org = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let version = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let object = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();

        let q = quarantine_key(org, object, Some("../../etc/passwd")).unwrap();
        let parsed_q = parse_key_for_org(&q.as_str(), org).unwrap();
        assert_eq!(parsed_q, q);
        assert_eq!(q.org_opaque().len(), OPAQUE_HEX_LEN);
        assert!(!q.as_str().contains("passwd"));
        assert!(q.as_str().starts_with("quarantine/"));

        let t = trusted_key(org, version, object, Some("/abs/path/file.pdf")).unwrap();
        let parsed_t = parse_key_for_org(&t.as_str(), org).unwrap();
        assert_eq!(parsed_t, t);
        authorize_key_for_version(&t, version).unwrap();
    }

    #[test]
    fn cross_org_parse_is_rejected() {
        let org_a = Uuid::new_v4();
        let org_b = Uuid::new_v4();
        let object = Uuid::new_v4();
        let key = quarantine_key(org_a, object, None).unwrap();
        assert!(matches!(
            parse_key_for_org(&key.as_str(), org_b),
            Err(StorageError::KeyOrgMismatch)
        ));
        assert!(matches!(
            authorize_key_for_org(&key, org_b),
            Err(StorageError::KeyOrgMismatch)
        ));
    }

    #[test]
    fn nil_identities_rejected() {
        let org = Uuid::new_v4();
        assert!(matches!(
            quarantine_key(Uuid::nil(), Uuid::new_v4(), None),
            Err(StorageError::MissingScope)
        ));
        assert!(matches!(
            trusted_key(org, Uuid::nil(), Uuid::new_v4(), None),
            Err(StorageError::MissingScope)
        ));
    }

    #[test]
    fn adversarial_filenames_never_enter_key() {
        let org = Uuid::new_v4();
        let version = Uuid::new_v4();
        let object = Uuid::new_v4();
        for name in ["../../etc/passwd", "/absolute/path", "file\nname.txt"] {
            let q = quarantine_key(org, object, Some(name)).unwrap();
            assert!(!q.as_str().contains(name));
            parse_key_for_org(&q.as_str(), org).unwrap();
            let t = trusted_key(org, version, object, Some(name)).unwrap();
            parse_key_for_org(&t.as_str(), org).unwrap();
        }
    }

    #[test]
    fn parse_rejects_malformed_and_cross_structure() {
        let org = Uuid::new_v4();
        for bad in [
            "",
            "/quarantine/aa/bb",
            "quarantine/../bb/cc",
            "quarantine/not-hex/33333333333333333333333333333333",
        ] {
            assert!(parse_key_for_org(bad, org).is_err());
        }
    }
}
