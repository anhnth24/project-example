//! Canonical built-in role / permission catalog (Phase 1C RBAC foundation).
//!
//! Loads `openapi/builtin-role-catalog.json` and validates matrix invariants.
//! The JSON fixture is the single grant-matrix source of truth for OpenAPI,
//! web presentation, and DB matrix tests.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinRoleCatalog {
    pub version: u32,
    pub roles: Vec<String>,
    pub permissions: Vec<BuiltinPermission>,
    pub grants: BTreeMap<String, Vec<String>>,
    pub restrictions: Vec<RoleRestriction>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleRestriction {
    pub id: String,
    pub description: String,
    pub enforced_by: String,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Active,
    Reserved,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinPermission {
    pub key: String,
    pub status: PermissionStatus,
    pub description: String,
    pub required_collection_access: Option<crate::db::models::AccessLevel>,
    pub conditional_policy: Option<String>,
    pub operation_refs: Vec<String>,
}

/// Deserialize the canonical built-in role catalog embedded at compile time.
pub fn load_builtin_role_catalog() -> BuiltinRoleCatalog {
    serde_json::from_str(include_str!("../../openapi/builtin-role-catalog.json"))
        .expect("builtin role catalog must be valid JSON")
}

/// Validate catalog structural and grant-matrix invariants.
///
/// Returns sorted error messages when invariants fail. Rejects duplicate roles,
/// duplicate permission keys, unknown grants, reserved grants, missing
/// operation references, active references to missing source files, reserved
/// references, and active keys lacking quoted-literal evidence in at least one
/// referenced source file.
pub fn validate_catalog_invariants(catalog: &BuiltinRoleCatalog) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut seen_roles = BTreeSet::new();
    for role in &catalog.roles {
        if !seen_roles.insert(role.as_str()) {
            errors.push(format!("duplicate role: {role}"));
        }
    }
    let role_set: BTreeSet<&str> = catalog.roles.iter().map(String::as_str).collect();

    let mut seen_keys = BTreeSet::new();
    let mut active_keys = BTreeSet::new();
    let mut reserved_keys = BTreeSet::new();
    for perm in &catalog.permissions {
        if !seen_keys.insert(perm.key.as_str()) {
            errors.push(format!("duplicate permission key: {}", perm.key));
        }
        match perm.status {
            PermissionStatus::Active => {
                active_keys.insert(perm.key.as_str());
                if perm.operation_refs.is_empty() {
                    errors.push(format!(
                        "active permission {} is missing operation references",
                        perm.key
                    ));
                } else {
                    let mut literal_found = false;
                    let needle = format!("\"{}\"", perm.key);
                    for rel in &perm.operation_refs {
                        let path = crate_root.join(rel);
                        if !path.is_file() {
                            errors.push(format!(
                                "active permission {} references missing source file: {rel}",
                                perm.key
                            ));
                            continue;
                        }
                        match std::fs::read_to_string(&path) {
                            Ok(contents) if contents.contains(&needle) => {
                                literal_found = true;
                            }
                            Ok(_) => {}
                            Err(err) => {
                                errors.push(format!(
                                    "active permission {} could not read source file {rel}: {err}",
                                    perm.key
                                ));
                            }
                        }
                    }
                    if !literal_found {
                        errors.push(format!(
                            "active permission {} has no operationRefs file containing quoted literal {}",
                            perm.key, needle
                        ));
                    }
                }
            }
            PermissionStatus::Reserved => {
                reserved_keys.insert(perm.key.as_str());
                if !perm.operation_refs.is_empty() {
                    errors.push(format!(
                        "reserved permission {} must not have operation references",
                        perm.key
                    ));
                }
            }
        }
    }

    let known_keys: BTreeSet<&str> = active_keys.union(&reserved_keys).copied().collect();
    for (role, keys) in &catalog.grants {
        if !role_set.contains(role.as_str()) {
            errors.push(format!("unknown grant role: {role}"));
        }
        for key in keys {
            if !known_keys.contains(key.as_str()) {
                errors.push(format!("unknown grant permission {key} for role {role}"));
                continue;
            }
            if reserved_keys.contains(key.as_str()) {
                errors.push(format!(
                    "reserved permission {key} must not be granted to role {role}"
                ));
            } else if !active_keys.contains(key.as_str()) {
                errors.push(format!(
                    "grant permission {key} for role {role} is not active"
                ));
            }
        }
    }

    errors.sort();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Current nine runtime (active) permission keys seeded in `permissions`.
pub const ACTIVE_RUNTIME_PERMISSIONS: &[&str] = &[
    "audit.view",
    "doc.delete",
    "doc.publish",
    "doc.quarantine.review",
    "doc.upload",
    "jobs.system",
    "member.manage",
    "qa.history",
    "qa.query",
];

/// Reserved permission keys that must remain ungranted and absent from runtime rows.
pub const RESERVED_PERMISSIONS: &[&str] =
    &["export.run", "intel.use", "pii.manage", "settings.manage"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_roles_are_canonical() {
        let catalog = load_builtin_role_catalog();
        assert_eq!(
            catalog.roles,
            vec![
                "owner".to_string(),
                "admin".to_string(),
                "editor".to_string(),
                "viewer".to_string()
            ]
        );
    }

    #[test]
    fn builtin_catalog_active_keys_match_runtime_permissions() {
        let catalog = load_builtin_role_catalog();
        let active: BTreeSet<&str> = catalog
            .permissions
            .iter()
            .filter(|p| p.status == PermissionStatus::Active)
            .map(|p| p.key.as_str())
            .collect();
        let expected: BTreeSet<&str> = ACTIVE_RUNTIME_PERMISSIONS.iter().copied().collect();
        assert_eq!(active, expected);
    }

    #[test]
    fn builtin_catalog_reserved_keys_are_canonical() {
        let catalog = load_builtin_role_catalog();
        let reserved: BTreeSet<&str> = catalog
            .permissions
            .iter()
            .filter(|p| p.status == PermissionStatus::Reserved)
            .map(|p| p.key.as_str())
            .collect();
        let expected: BTreeSet<&str> = RESERVED_PERMISSIONS.iter().copied().collect();
        assert_eq!(reserved, expected);
    }

    #[test]
    fn every_grant_references_an_active_key() {
        let catalog = load_builtin_role_catalog();
        let active: BTreeSet<&str> = catalog
            .permissions
            .iter()
            .filter(|p| p.status == PermissionStatus::Active)
            .map(|p| p.key.as_str())
            .collect();
        for (role, keys) in &catalog.grants {
            for key in keys {
                assert!(
                    active.contains(key.as_str()),
                    "grant {key} for role {role} is not an active permission"
                );
            }
        }
    }

    #[test]
    fn no_reserved_key_is_granted() {
        let catalog = load_builtin_role_catalog();
        let reserved: BTreeSet<&str> = catalog
            .permissions
            .iter()
            .filter(|p| p.status == PermissionStatus::Reserved)
            .map(|p| p.key.as_str())
            .collect();
        for (role, keys) in &catalog.grants {
            for key in keys {
                assert!(
                    !reserved.contains(key.as_str()),
                    "reserved key {key} must not be granted to {role}"
                );
            }
        }
    }

    #[test]
    fn doc_quarantine_review_has_zero_default_grants() {
        let catalog = load_builtin_role_catalog();
        for (role, keys) in &catalog.grants {
            assert!(
                !keys.iter().any(|k| k == "doc.quarantine.review"),
                "doc.quarantine.review must have zero default grants; found on {role}"
            );
        }
    }

    #[test]
    fn editor_has_no_doc_delete() {
        let catalog = load_builtin_role_catalog();
        let editor = catalog
            .grants
            .get("editor")
            .expect("editor grants must exist");
        assert!(
            !editor.iter().any(|k| k == "doc.delete"),
            "built-in editor must not have doc.delete"
        );
    }

    #[test]
    fn every_active_key_has_operation_refs() {
        let catalog = load_builtin_role_catalog();
        for perm in catalog
            .permissions
            .iter()
            .filter(|p| p.status == PermissionStatus::Active)
        {
            assert!(
                !perm.operation_refs.is_empty(),
                "active key {} must have at least one operationRefs entry",
                perm.key
            );
        }
    }

    #[test]
    fn every_reserved_key_has_no_operation_refs() {
        let catalog = load_builtin_role_catalog();
        for perm in catalog
            .permissions
            .iter()
            .filter(|p| p.status == PermissionStatus::Reserved)
        {
            assert!(
                perm.operation_refs.is_empty(),
                "reserved key {} must have no operationRefs",
                perm.key
            );
        }
    }

    #[test]
    fn validate_catalog_invariants_api_accepts_loaded_catalog() {
        let catalog = load_builtin_role_catalog();
        let result = validate_catalog_invariants(&catalog);
        assert!(
            result.is_ok(),
            "canonical catalog must satisfy invariants: {:?}",
            result.err()
        );
    }
}
