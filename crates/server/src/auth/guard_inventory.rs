//! Machine-checked route/service/audit guard inventory (Phase 1C PR 3).
//!
//! Canonical fixture: `openapi/guard-inventory.json`. Each OpenAPI operation and
//! each [`crate::api::ROUTE_INVENTORY`] method/path must have exactly one row.
//! `requiredCollectionAccess` is derived from `builtin-role-catalog.json` and
//! must never be stored in the guard fixture.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::api::{openapi_operation_inventory, ROUTE_INVENTORY};
use crate::auth::rbac_catalog::{load_builtin_role_catalog, BuiltinRoleCatalog, PermissionStatus};
use crate::db::models::AccessLevel;

/// Relative path (from the server crate root) of the canonical guard fixture.
pub const GUARD_INVENTORY_REL_PATH: &str = "openapi/guard-inventory.json";

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardInventory {
    pub version: u32,
    pub operations: Vec<GuardOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardOperation {
    pub operation_id: String,
    pub authz_kind: AuthzKind,
    #[serde(default)]
    pub permission: Option<String>,
    pub route: GuardRoute,
    pub service_entry: String,
    pub identity_kind: IdentityKind,
    pub mutation: bool,
    pub audit: AuditDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardRoute {
    pub method: String,
    pub path: String,
}

/// Authorization classification for a guarded operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthzKind {
    Public,
    Authenticated,
    Permission,
    Capability,
    System,
}

/// Principal class expected at the service entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IdentityKind {
    User,
    Worker,
    System,
    Capability,
}

/// Mutation audit coverage disposition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditDisposition {
    pub status: AuditStatus,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuditStatus {
    Required,
    #[serde(rename = "na")]
    Na,
}

/// Absolute path to the guard inventory fixture for this crate.
pub fn guard_inventory_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(GUARD_INVENTORY_REL_PATH)
}

/// Load and deserialize the canonical guard inventory from disk.
///
/// Uses a filesystem read (not `include_str!`) so the RED completeness suite can
/// compile before the fixture exists; GREEN fills `openapi/guard-inventory.json`.
pub fn load_guard_inventory() -> Result<GuardInventory, String> {
    let path = guard_inventory_path();
    let raw = std::fs::read_to_string(&path).map_err(|err| {
        format!(
            "missing guard inventory fixture at {}: {err}",
            path.display()
        )
    })?;
    parse_guard_inventory_json(&raw)
}

/// Parse guard inventory JSON, rejecting any embedded `requiredCollectionAccess`.
pub fn parse_guard_inventory_json(raw: &str) -> Result<GuardInventory, String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| format!("guard inventory JSON is invalid: {err}"))?;
    if let Some(path) = find_required_collection_access_path(&value, "$") {
        return Err(format!(
            "guard inventory must not store requiredCollectionAccess (derived from builtin-role-catalog.json); found at {path}"
        ));
    }
    serde_json::from_value(value).map_err(|err| format!("guard inventory schema is invalid: {err}"))
}

fn find_required_collection_access_path(value: &serde_json::Value, path: &str) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if map.contains_key("requiredCollectionAccess") {
                return Some(format!("{path}.requiredCollectionAccess"));
            }
            for (key, child) in map {
                if let Some(found) =
                    find_required_collection_access_path(child, &format!("{path}.{key}"))
                {
                    return Some(found);
                }
            }
            None
        }
        serde_json::Value::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                if let Some(found) =
                    find_required_collection_access_path(child, &format!("{path}[{idx}]"))
                {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

/// Derive collection access for a permission key from the RBAC catalog.
///
/// Returns `None` when the permission is unknown or declares no collection scope.
pub fn derived_required_collection_access(
    catalog: &BuiltinRoleCatalog,
    permission: &str,
) -> Option<AccessLevel> {
    catalog
        .permissions
        .iter()
        .find(|p| p.key == permission)
        .and_then(|p| p.required_collection_access)
}

/// Structural / permission / audit invariants for a loaded inventory.
///
/// Completeness against OpenAPI and [`ROUTE_INVENTORY`] is
/// [`validate_guard_completeness`].
pub fn validate_guard_inventory_invariants(
    inventory: &GuardInventory,
    catalog: &BuiltinRoleCatalog,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    let mut active_keys = BTreeSet::new();
    let mut reserved_keys = BTreeSet::new();
    for perm in &catalog.permissions {
        match perm.status {
            PermissionStatus::Active => {
                active_keys.insert(perm.key.as_str());
            }
            PermissionStatus::Reserved => {
                reserved_keys.insert(perm.key.as_str());
            }
        }
    }

    for op in &inventory.operations {
        let label = op.operation_id.as_str();

        if op.route.method.trim().is_empty() || op.route.path.trim().is_empty() {
            errors.push(format!(
                "operation {label} is missing route.method/route.path"
            ));
        }
        if op.service_entry.trim().is_empty() {
            errors.push(format!("operation {label} is missing serviceEntry"));
        }

        match op.authz_kind {
            AuthzKind::Permission => {
                match op.permission.as_deref() {
                    None | Some("") => {
                        errors.push(format!(
                            "permission operation {label} must declare an active permission key"
                        ));
                    }
                    Some(key) if reserved_keys.contains(key) => {
                        errors.push(format!(
                            "permission operation {label} references reserved permission {key}"
                        ));
                    }
                    Some(key) if !active_keys.contains(key) => {
                        errors.push(format!(
                            "permission operation {label} references unknown or inactive permission {key}"
                        ));
                    }
                    Some(_) => {}
                }
                // Permission rows must carry an exact route + service pair (no silent omit).
                if op.route.method.trim().is_empty()
                    || op.route.path.trim().is_empty()
                    || !op.service_entry.contains("::")
                {
                    errors.push(format!(
                        "permission operation {label} must include exact route method/path and a serviceEntry path (module::fn)"
                    ));
                }
            }
            _ => {
                if let Some(key) = op.permission.as_deref() {
                    errors.push(format!(
                        "operation {label} has authzKind {:?} but still declares permission {key}",
                        op.authz_kind
                    ));
                }
            }
        }

        // Every inventory row must declare audit semantics (required+action or na+reason).
        // Serde requires the `audit` object; these rules reject hollow dispositions.
        match op.audit.status {
            AuditStatus::Required => match op.audit.action.as_deref() {
                None | Some("") => {
                    errors.push(format!(
                        "operation {label} audit.status=required but audit.action is missing"
                    ));
                }
                Some(_) => {
                    if op
                        .audit
                        .reason
                        .as_deref()
                        .is_some_and(|r| !r.trim().is_empty())
                    {
                        errors.push(format!(
                            "operation {label} audit.status=required must not set audit.reason"
                        ));
                    }
                }
            },
            AuditStatus::Na => match op.audit.reason.as_deref() {
                None | Some("") => {
                    errors.push(format!(
                        "operation {label} audit.status=na but documented non-sensitive reason is missing"
                    ));
                }
                Some(_) => {
                    if op
                        .audit
                        .action
                        .as_deref()
                        .is_some_and(|a| !a.trim().is_empty())
                    {
                        errors.push(format!(
                            "operation {label} audit.status=na must not set audit.action"
                        ));
                    }
                }
            },
        }
    }

    errors.sort();
    errors.dedup();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Completeness: every OpenAPI operationId and every inventory route has exactly
/// one guard row. Failures enumerate missing and duplicate keys (never a bare count).
pub fn validate_guard_completeness(
    inventory: &GuardInventory,
    openapi_ops: &[(String, String, String)],
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    let mut by_operation_id: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_route: BTreeMap<(String, String), usize> = BTreeMap::new();
    for op in &inventory.operations {
        *by_operation_id.entry(op.operation_id.as_str()).or_insert(0) += 1;
        let route_key = (op.route.method.to_ascii_lowercase(), op.route.path.clone());
        *by_route.entry(route_key).or_insert(0) += 1;
    }

    for (operation_id, count) in &by_operation_id {
        if *count > 1 {
            errors.push(format!(
                "duplicate guard operationId: {operation_id} ({count} rows)"
            ));
        }
    }
    for ((method, path), count) in &by_route {
        if *count > 1 {
            errors.push(format!(
                "duplicate guard route: {method} {path} ({count} rows)"
            ));
        }
    }

    let openapi_ids: BTreeSet<&str> = openapi_ops
        .iter()
        .map(|(_, _, operation_id)| operation_id.as_str())
        .collect();
    let openapi_by_id: BTreeMap<&str, (&str, &str)> = openapi_ops
        .iter()
        .map(|(method, path, operation_id)| {
            (operation_id.as_str(), (method.as_str(), path.as_str()))
        })
        .collect();
    let openapi_routes: BTreeSet<(&str, &str)> = openapi_ops
        .iter()
        .map(|(method, path, _)| (method.as_str(), path.as_str()))
        .collect();
    let inventory_routes: BTreeSet<(&str, &str)> = ROUTE_INVENTORY
        .iter()
        .map(|&(method, path, _)| (method, path))
        .collect();

    for op in &inventory.operations {
        if let Some(&(method, path)) = openapi_by_id.get(op.operation_id.as_str()) {
            let guard_method = op.route.method.to_ascii_lowercase();
            if guard_method != method || op.route.path != path {
                errors.push(format!(
                    "operation {} route/service pair mismatch: guard route is {} {} but OpenAPI is {} {}",
                    op.operation_id, op.route.method, op.route.path, method, path
                ));
            }
        }
    }

    for operation_id in &openapi_ids {
        match by_operation_id.get(operation_id).copied().unwrap_or(0) {
            0 => errors.push(format!("missing guard operationId: {operation_id}")),
            1 => {}
            count => errors.push(format!(
                "duplicate guard operationId: {operation_id} ({count} rows)"
            )),
        }
    }

    for &(method, path, _) in ROUTE_INVENTORY {
        let key = (method.to_string(), path.to_string());
        match by_route.get(&key).copied().unwrap_or(0) {
            0 => errors.push(format!("missing guard route: {method} {path}")),
            1 => {}
            count => errors.push(format!(
                "duplicate guard route: {method} {path} ({count} rows)"
            )),
        }
    }

    for op in &inventory.operations {
        if !openapi_ids.contains(op.operation_id.as_str()) {
            errors.push(format!(
                "orphan guard operationId not in OpenAPI: {}",
                op.operation_id
            ));
        }
        let method = op.route.method.to_ascii_lowercase();
        if !inventory_routes.contains(&(method.as_str(), op.route.path.as_str()))
            && !openapi_routes.contains(&(method.as_str(), op.route.path.as_str()))
        {
            errors.push(format!(
                "orphan guard route not in ROUTE_INVENTORY: {} {}",
                op.route.method, op.route.path
            ));
        }
    }

    // Deduplicate messages that may appear from both the local-count and expected-set passes.
    errors.sort();
    errors.dedup();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::embedded_openapi_yaml;

    fn sample_permission_row() -> &'static str {
        r#"{
          "operationId": "publishDocumentVersion",
          "authzKind": "permission",
          "permission": "doc.publish",
          "route": {
            "method": "post",
            "path": "/documents/{documentId}/versions/{versionId}/publish"
          },
          "serviceEntry": "db::document_versions::publish_version",
          "identityKind": "user",
          "mutation": true,
          "audit": {"status": "required", "action": "document.publish"}
        }"#
    }

    #[test]
    fn invariants_reject_permission_row_without_service_path() {
        let raw = format!(
            r#"{{"version":1,"operations":[{}]}}"#,
            sample_permission_row().replace(
                r#""serviceEntry": "db::document_versions::publish_version""#,
                r#""serviceEntry": "publish_version""#
            )
        );
        let inventory = parse_guard_inventory_json(&raw).expect("schema ok");
        let catalog = load_builtin_role_catalog();
        let errors = validate_guard_inventory_invariants(&inventory, &catalog)
            .expect_err("serviceEntry path required");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("exact route method/path and a serviceEntry path")),
            "expected serviceEntry path failure, got: {errors:?}"
        );
    }

    #[test]
    fn parse_rejects_embedded_required_collection_access() {
        let raw = r#"{
          "version": 1,
          "operations": [{
            "operationId": "publishDocumentVersion",
            "authzKind": "permission",
            "permission": "doc.publish",
            "requiredCollectionAccess": "write",
            "route": {
              "method": "post",
              "path": "/documents/{documentId}/versions/{versionId}/publish"
            },
            "serviceEntry": "db::document_versions::publish_version",
            "identityKind": "user",
            "mutation": true,
            "audit": {"status": "required", "action": "document.publish"}
          }]
        }"#;
        let err = parse_guard_inventory_json(raw).expect_err("must reject");
        assert!(
            err.contains("requiredCollectionAccess"),
            "expected requiredCollectionAccess rejection, got: {err}"
        );
    }

    #[test]
    fn invariants_reject_reserved_permission_keys() {
        let raw = format!(
            r#"{{"version":1,"operations":[{}]}}"#,
            sample_permission_row().replace("doc.publish", "export.run")
        );
        let inventory = parse_guard_inventory_json(&raw).expect("schema ok");
        let catalog = load_builtin_role_catalog();
        let errors = validate_guard_inventory_invariants(&inventory, &catalog)
            .expect_err("reserved must fail");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("reserved permission export.run")),
            "expected reserved rejection, got: {errors:?}"
        );
    }

    #[test]
    fn invariants_require_audit_action_or_na_reason_for_mutations() {
        let raw = format!(
            r#"{{"version":1,"operations":[{}]}}"#,
            sample_permission_row().replace(
                r#""audit": {"status": "required", "action": "document.publish"}"#,
                r#""audit": {"status": "na"}"#
            )
        );
        let inventory = parse_guard_inventory_json(&raw).expect("schema ok");
        let catalog = load_builtin_role_catalog();
        let errors = validate_guard_inventory_invariants(&inventory, &catalog)
            .expect_err("na without reason must fail");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("documented non-sensitive reason is missing")),
            "expected na-reason failure, got: {errors:?}"
        );
    }

    #[test]
    fn derived_collection_access_comes_from_role_catalog() {
        let catalog = load_builtin_role_catalog();
        assert_eq!(
            derived_required_collection_access(&catalog, "doc.publish"),
            Some(AccessLevel::Write)
        );
        assert_eq!(
            derived_required_collection_access(&catalog, "doc.delete"),
            Some(AccessLevel::Admin)
        );
        assert_eq!(
            derived_required_collection_access(&catalog, "member.manage"),
            None
        );
    }

    #[test]
    fn completeness_enumerates_missing_and_duplicate_operations() {
        let inventory = GuardInventory {
            version: 1,
            operations: vec![
                serde_json::from_str(sample_permission_row()).unwrap(),
                serde_json::from_str(sample_permission_row()).unwrap(),
            ],
        };
        let openapi_ops = openapi_operation_inventory(embedded_openapi_yaml());
        let errors = validate_guard_completeness(&inventory, &openapi_ops)
            .expect_err("incomplete inventory must fail");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("duplicate guard operationId: publishDocumentVersion")),
            "expected duplicate enumeration, got: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("missing guard operationId: ")),
            "expected missing operationId enumeration, got: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.starts_with("missing guard route: ")),
            "expected missing route enumeration, got: {errors:?}"
        );
        // Must not collapse to a bare count.
        assert!(
            !errors
                .iter()
                .any(|e| e == "missing 59 operations" || e.starts_with("missing count")),
            "completeness must enumerate ids/routes, not bare counts: {errors:?}"
        );
    }

    #[test]
    fn guard_inventory_covers_every_openapi_operation_and_route() {
        let catalog = load_builtin_role_catalog();
        let openapi_ops = openapi_operation_inventory(embedded_openapi_yaml());
        assert!(
            !openapi_ops.is_empty(),
            "OpenAPI operation inventory must be non-empty"
        );
        assert_eq!(
            openapi_ops.len(),
            ROUTE_INVENTORY.len(),
            "every ROUTE_INVENTORY entry must expose an operationId in OpenAPI"
        );

        let mut errors = Vec::new();
        let inventory = match load_guard_inventory() {
            Ok(inventory) => inventory,
            Err(load_err) => {
                errors.push(load_err);
                GuardInventory {
                    version: 1,
                    operations: Vec::new(),
                }
            }
        };

        if let Err(invariant_errors) = validate_guard_inventory_invariants(&inventory, &catalog) {
            errors.extend(invariant_errors);
        }
        if let Err(completeness_errors) = validate_guard_completeness(&inventory, &openapi_ops) {
            errors.extend(completeness_errors);
        }

        errors.sort();
        errors.dedup();
        assert!(
            errors.is_empty(),
            "guard inventory incomplete (enumerate missing/duplicate ops):\n{}",
            errors.join("\n")
        );
    }
}
