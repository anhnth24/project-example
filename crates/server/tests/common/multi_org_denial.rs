//! Phase 1C unified multi-org denial manifest, fixture, and shared-world helpers.
//!
//! RED: validator + schema types compile; [`validate_denial_manifest`] enumerates
//! missing business operations/routes/test joins. GREEN fills the fixture/manifest
//! and implements [`MultiOrgDenialWorld::boot`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fileconv_server::api::ROUTE_INVENTORY;
use fileconv_server::auth::guard_inventory::{
    load_guard_inventory, AuthzKind, GuardInventory, GuardOperation,
};

pub const DENIAL_FIXTURE_REL_PATH: &str = "tests/fixtures/multi-org-denial.fixture.json";
pub const DENIAL_MANIFEST_REL_PATH: &str = "tests/fixtures/multi-org-denial.manifest.json";
pub const DENIAL_NA_EVIDENCE_REL_PATH: &str = "tests/fixtures/multi-org-denial.na-evidence.json";

/// Stable N/A category ids — exactly the five Phase 1C dispositions.
pub const NA_CATEGORY_EXPORT_ROUTE_ABSENT: &str = "export_route_absent";
pub const NA_CATEGORY_AUTOCOMPLETE_ROUTE_ABSENT: &str = "autocomplete_route_absent";
pub const NA_CATEGORY_SIGNED_URL_CAPABILITY_SUBSTITUTION: &str =
    "signed_url_capability_substitution";
pub const NA_CATEGORY_RESERVED_PERMISSION_NO_RUNTIME: &str = "reserved_permission_no_runtime";
pub const NA_CATEGORY_EMBEDDING_TOKEN_METERING_LOCAL_MOCK: &str =
    "embedding_token_metering_local_mock";

pub const REQUIRED_NA_CATEGORIES: &[&str] = &[
    NA_CATEGORY_EXPORT_ROUTE_ABSENT,
    NA_CATEGORY_AUTOCOMPLETE_ROUTE_ABSENT,
    NA_CATEGORY_SIGNED_URL_CAPABILITY_SUBSTITUTION,
    NA_CATEGORY_RESERVED_PERMISSION_NO_RUNTIME,
    NA_CATEGORY_EMBEDDING_TOKEN_METERING_LOCAL_MOCK,
];

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialFixture {
    pub version: u32,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub orgs: Vec<DenialFixtureOrg>,
    pub users_per_org: u32,
    #[serde(default)]
    pub role_topology: Vec<String>,
    pub collection_visibilities: Vec<String>,
    pub duplicate_names: DenialDuplicateNames,
    #[serde(default)]
    pub indexed_markers: BTreeMap<String, String>,
    #[serde(default)]
    pub object_key_template: String,
    pub pre_revoke_tokens: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialFixtureOrg {
    pub key: String,
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialDuplicateNames {
    pub collection: String,
    pub document: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialManifest {
    pub version: u32,
    pub rows: Vec<DenialManifestRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialManifestRow {
    pub id: String,
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default)]
    pub test_name: Option<String>,
    #[serde(default)]
    pub operation_id: Option<String>,
    pub guard_inventory_ref: String,
    pub layer: DenialLayer,
    pub status: DenialRowStatus,
    #[serde(default)]
    pub na_category: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialLayer {
    Http,
    Service,
    Repository,
    Storage,
    Cache,
    Worker,
    Sse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenialRowStatus {
    Executable,
    Na,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialNaEvidence {
    pub version: u32,
    pub categories: Vec<DenialNaCategory>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DenialNaCategory {
    pub id: String,
    pub description: String,
    pub proof_kind: NaProofKind,
    #[serde(default)]
    pub permission_keys: Vec<String>,
    #[serde(default)]
    pub operation_ids: Vec<String>,
    #[serde(default)]
    pub scan: Option<NaSourceScanProof>,
    #[serde(default)]
    pub substitution: Option<NaCapabilitySubstitutionProof>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NaProofKind {
    SourceScan,
    CapabilitySubstitution,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaSourceScanProof {
    #[serde(default)]
    pub absent_route_path_patterns: Vec<String>,
    #[serde(default)]
    pub absent_operation_id_patterns: Vec<String>,
    #[serde(default)]
    pub catalog_permission_status: Option<String>,
    #[serde(default)]
    pub environment_profile: Option<String>,
    #[serde(default)]
    pub embedding_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NaCapabilitySubstitutionProof {
    pub binary: String,
    pub test_names: Vec<String>,
    #[serde(default)]
    pub absent_route_path_patterns: Vec<String>,
}

/// Foreign org markers scanned by [`assert_denial_no_leak`].
#[derive(Debug, Clone, Default)]
pub struct ForeignMarkers {
    pub org_ids: Vec<String>,
    pub user_ids: Vec<String>,
    pub collection_ids: Vec<String>,
    pub document_ids: Vec<String>,
    pub version_ids: Vec<String>,
    pub job_ids: Vec<String>,
    pub conflict_ids: Vec<String>,
    pub object_keys: Vec<String>,
    pub names: Vec<String>,
    pub marker_strings: Vec<String>,
}

/// HTTP denial semantics expected from production routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenialExpectation {
    /// Successful or non-denial response must not leak foreign markers.
    AllowSuccess,
    /// Foreign id in request body scope → 403 without foreign markers.
    BodyScopeForbidden,
    /// Addressed foreign resource → 404 without existence oracle.
    PathIdorNotFound,
    /// Missing/invalid session → 401.
    Unauthorized,
}

/// Minimal HTTP response view for leakage assertions.
#[derive(Debug, Clone)]
pub struct DenialResponse<'a> {
    pub status: u16,
    pub body: &'a [u8],
    pub headers: Vec<(&'a str, &'a str)>,
}

use crate::common::{DualRoleEphemeralDb, MinioCleanupGuard};

/// Shared multi-org world for executable denial tests.
pub struct MultiOrgDenialWorld {
    pub fixture: DenialFixture,
    pub(crate) ephemeral: DualRoleEphemeralDb,
    pub pool: deadpool_postgres::Pool,
    pub app: axum::Router,
    pub(crate) store: Option<fileconv_server::storage::minio::MinioClient>,
    pub(crate) minio_guard: Option<MinioCleanupGuard>,
    pub orgs: BTreeMap<String, crate::common::multi_org_denial_world::BootedOrg>,
}

impl MultiOrgDenialWorld {
    pub async fn boot() -> Self {
        crate::common::multi_org_denial_world::boot_world().await
    }

    pub fn foreign_markers_for(&self, actor_org_key: &str) -> ForeignMarkers {
        crate::common::multi_org_denial_world::foreign_markers_for(self, actor_org_key)
    }

    pub async fn cleanup(self) -> Result<(), String> {
        crate::common::multi_org_denial_world::cleanup_world(self).await
    }

    pub fn assert_base_topology(&self) {
        crate::common::multi_org_denial_world::assert_base_topology(self);
    }
}

/// Assert denial responses contain no foreign IDs, names, keys, or marker strings.
pub fn assert_denial_no_leak(
    response: &DenialResponse<'_>,
    foreign_markers: &ForeignMarkers,
    expectation: DenialExpectation,
) {
    let body = String::from_utf8_lossy(response.body);
    let body_lower = body.to_lowercase();
    let mut leaks = Vec::new();

    match expectation {
        DenialExpectation::AllowSuccess => {
            if !(200..300).contains(&response.status) && response.status != 401 {
                leaks.push(format!(
                    "expected success response, got {}",
                    response.status
                ));
            }
        }
        DenialExpectation::BodyScopeForbidden => {
            if response.status != 403 {
                leaks.push(format!(
                    "body-scope denial expected 403, got {}",
                    response.status
                ));
            }
        }
        DenialExpectation::PathIdorNotFound => {
            if response.status != 404 {
                leaks.push(format!(
                    "path-IDOR denial expected 404, got {}",
                    response.status
                ));
            }
        }
        DenialExpectation::Unauthorized => {
            if response.status != 401 {
                leaks.push(format!(
                    "unauthorized denial expected 401, got {}",
                    response.status
                ));
            }
        }
    }

    if (200..300).contains(&response.status)
        && !matches!(expectation, DenialExpectation::AllowSuccess)
    {
        leaks.push(format!(
            "denial must not return success status {}",
            response.status
        ));
    }
    if response.status >= 500 {
        leaks.push(format!(
            "denial must not surface as server error: status {}",
            response.status
        ));
    }

    for needle in foreign_markers.all_needles() {
        if body_lower.contains(&needle.to_lowercase()) {
            leaks.push(format!("body contains foreign marker: {needle}"));
        }
        for (name, value) in &response.headers {
            if value.to_lowercase().contains(&needle.to_lowercase()) {
                leaks.push(format!("header {name} contains foreign marker: {needle}"));
            }
        }
    }

    if !leaks.is_empty() {
        leaks.sort();
        panic!(
            "assert_denial_no_leak failed (status {}):\n{}",
            response.status,
            leaks.join("\n")
        );
    }
}

impl ForeignMarkers {
    pub fn all_strings(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for slice in [
            &self.org_ids,
            &self.user_ids,
            &self.collection_ids,
            &self.document_ids,
            &self.version_ids,
            &self.job_ids,
            &self.conflict_ids,
            &self.object_keys,
            &self.names,
            &self.marker_strings,
        ] {
            for item in slice {
                out.push(item.as_str());
            }
        }
        out
    }

    /// Canonical and normalized variants for case/substring-resistant scanning.
    pub fn all_needles(&self) -> Vec<String> {
        let mut needles = BTreeSet::new();
        for value in self.all_strings() {
            needles.insert(value.to_string());
            needles.insert(value.to_lowercase());
            needles.insert(value.to_uppercase());
            if let Ok(uuid) = uuid::Uuid::parse_str(value) {
                needles.insert(uuid.simple().to_string());
                needles.insert(uuid.to_string());
            }
        }
        needles.into_iter().collect()
    }
}

pub fn denial_fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(DENIAL_FIXTURE_REL_PATH)
}

pub fn denial_manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(DENIAL_MANIFEST_REL_PATH)
}

pub fn denial_na_evidence_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(DENIAL_NA_EVIDENCE_REL_PATH)
}

pub fn tests_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

pub fn load_denial_fixture() -> Result<DenialFixture, String> {
    let path = denial_fixture_path();
    let raw = std::fs::read_to_string(&path)
        .map_err(|err| format!("missing denial fixture at {}: {err}", path.display()))?;
    serde_json::from_str(&raw).map_err(|err| format!("denial fixture JSON invalid: {err}"))
}

pub fn load_denial_manifest() -> Result<DenialManifest, String> {
    let path = denial_manifest_path();
    let raw = std::fs::read_to_string(&path)
        .map_err(|err| format!("missing denial manifest at {}: {err}", path.display()))?;
    serde_json::from_str(&raw).map_err(|err| format!("denial manifest JSON invalid: {err}"))
}

pub fn load_denial_na_evidence() -> Result<DenialNaEvidence, String> {
    let path = denial_na_evidence_path();
    let raw = std::fs::read_to_string(&path)
        .map_err(|err| format!("missing denial N/A evidence at {}: {err}", path.display()))?;
    serde_json::from_str(&raw).map_err(|err| format!("denial N/A evidence JSON invalid: {err}"))
}

/// Business operations require denial evidence; public probes and unauthenticated auth
/// entrypoints are excluded consistently with guard-inventory `authzKind`.
pub fn is_business_guard_operation(op: &GuardOperation) -> bool {
    !matches!(op.authz_kind, AuthzKind::Public)
}

pub fn business_guard_operations(inventory: &GuardInventory) -> Vec<&GuardOperation> {
    inventory
        .operations
        .iter()
        .filter(|op| is_business_guard_operation(op))
        .collect()
}

/// Join ROUTE_INVENTORY routes to guard rows; only business-classified routes need denial rows.
pub fn business_route_inventory(
    inventory: &GuardInventory,
) -> Vec<(&str, &str, &GuardOperation)> {
    let by_route: BTreeMap<(String, String), &GuardOperation> = inventory
        .operations
        .iter()
        .map(|op| {
            (
                (op.route.method.to_ascii_lowercase(), op.route.path.clone()),
                op,
            )
        })
        .collect();

    ROUTE_INVENTORY
        .iter()
        .filter_map(|&(method, path, _)| {
            let key = (method.to_string(), path.to_string());
            let guard = by_route.get(&key)?;
            if is_business_guard_operation(guard) {
                Some((method, path, *guard))
            } else {
                None
            }
        })
        .collect()
}

/// Scan `tests/{binary}.rs` for `fn {name}` / `async fn {name}` registrations.
pub fn registered_test_functions(binary: &str) -> Result<BTreeSet<String>, String> {
    let path = tests_root().join(format!("{binary}.rs"));
    let raw = std::fs::read_to_string(&path).map_err(|err| {
        format!(
            "integration test source missing for binary {binary} at {}: {err}",
            path.display()
        )
    })?;
    Ok(extract_rust_test_function_names(&raw))
}

fn extract_rust_test_function_names(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        let rest = trimmed
            .strip_prefix("async fn ")
            .or_else(|| trimmed.strip_prefix("fn "))
            .or_else(|| trimmed.strip_prefix("pub async fn "))
            .or_else(|| trimmed.strip_prefix("pub fn "));
        if let Some(rest) = rest {
            if let Some(name) = rest
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
            {
                if !name.is_empty() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
}

/// Validate denial manifest joins guard inventory, business routes, test sources, and N/A proof.
pub fn validate_denial_manifest(
    inventory: &GuardInventory,
    manifest: &DenialManifest,
    na_evidence: &DenialNaEvidence,
    fixture: &DenialFixture,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    validate_fixture_shape(fixture, &mut errors);
    validate_na_evidence_shape(na_evidence, inventory, &mut errors);

    let business_ops: BTreeMap<&str, &GuardOperation> = business_guard_operations(inventory)
        .into_iter()
        .map(|op| (op.operation_id.as_str(), op))
        .collect();
    let business_routes = business_route_inventory(inventory);

    let mut rows_by_id: BTreeMap<&str, usize> = BTreeMap::new();
    let mut covered_operation_ids: BTreeSet<String> = BTreeSet::new();
    let mut covered_routes: BTreeSet<(String, String)> = BTreeSet::new();
    let mut test_cache: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    let mut na_categories_manifest: BTreeSet<String> = BTreeSet::new();

    for row in &manifest.rows {
        *rows_by_id.entry(row.id.as_str()).or_insert(0) += 1;

        if let Some(operation_id) = row.operation_id.as_deref() {
            if !business_ops.contains_key(operation_id) {
                errors.push(format!(
                    "manifest row {} references non-business or unknown operationId: {operation_id}",
                    row.id
                ));
            }
        }

        if row.guard_inventory_ref.is_empty() {
            errors.push(format!(
                "manifest row {} is missing guardInventoryRef",
                row.id
            ));
        } else if matches!(row.status, DenialRowStatus::Executable)
            && !inventory
                .operations
                .iter()
                .any(|op| op.operation_id == row.guard_inventory_ref)
        {
            errors.push(format!(
                "manifest row {} references unknown guardInventoryRef: {}",
                row.id, row.guard_inventory_ref
            ));
        }

        match row.status {
            DenialRowStatus::Executable => {
                if let Some(operation_id) = &row.operation_id {
                    if row.guard_inventory_ref != *operation_id {
                        errors.push(format!(
                            "executable manifest row {} guardInventoryRef {} must equal operationId {operation_id}",
                            row.id, row.guard_inventory_ref
                        ));
                    }
                } else {
                    errors.push(format!(
                        "executable manifest row {} must declare operationId",
                        row.id
                    ));
                }

                let (binary, test_name) = match (&row.binary, &row.test_name) {
                    (Some(binary), Some(test_name))
                        if !binary.trim().is_empty() && !test_name.trim().is_empty() =>
                    {
                        (binary.as_str(), test_name.as_str())
                    }
                    _ => {
                        errors.push(format!(
                            "executable manifest row {} must declare binary and testName",
                            row.id
                        ));
                        continue;
                    }
                };

                let registered = test_cache.entry(binary.to_string()).or_insert_with(|| {
                    registered_test_functions(binary).unwrap_or_else(|err| {
                        errors.push(err);
                        BTreeSet::new()
                    })
                });
                if !registered.contains(test_name) {
                    errors.push(format!(
                        "executable manifest row {} references unknown test source: {binary}::{test_name}",
                        row.id
                    ));
                }

                if let Some(operation_id) = &row.operation_id {
                    covered_operation_ids.insert(operation_id.clone());
                    if let Some(guard) = business_ops.get(operation_id.as_str()) {
                        covered_routes.insert((
                            guard.route.method.to_ascii_lowercase(),
                            guard.route.path.clone(),
                        ));
                    }
                }
            }
            DenialRowStatus::Na => {
                let category = match row.na_category.as_deref() {
                    Some(category) if !category.trim().is_empty() => category.to_string(),
                    _ => {
                        errors.push(format!(
                            "N/A manifest row {} must declare naCategory",
                            row.id
                        ));
                        continue;
                    }
                };
                na_categories_manifest.insert(category.clone());
                if row.guard_inventory_ref != category {
                    errors.push(format!(
                        "N/A manifest row {} guardInventoryRef must equal naCategory {category}",
                        row.id
                    ));
                }
                if row.binary.is_some() || row.test_name.is_some() || row.operation_id.is_some() {
                    errors.push(format!(
                        "N/A manifest row {} must not declare binary/testName/operationId",
                        row.id
                    ));
                }
                if !na_evidence
                    .categories
                    .iter()
                    .any(|entry| entry.id == category)
                {
                    errors.push(format!(
                        "N/A manifest row {} references unknown naCategory: {category}",
                        row.id
                    ));
                }
            }
        }
    }

    for required in REQUIRED_NA_CATEGORIES {
        if !na_categories_manifest.contains(*required) {
            errors.push(format!("missing N/A manifest row for category: {required}"));
        }
    }
    if na_categories_manifest.len() != REQUIRED_NA_CATEGORIES.len() {
        errors.push(format!(
            "manifest must contain exactly {} N/A rows, found {}",
            REQUIRED_NA_CATEGORIES.len(),
            na_categories_manifest.len()
        ));
    }

    for (id, count) in &rows_by_id {
        if *count > 1 {
            errors.push(format!("duplicate manifest row id: {id} ({count} rows)"));
        }
    }

    for operation_id in business_ops.keys() {
        if !covered_operation_ids.contains(*operation_id) {
            errors.push(format!(
                "missing denial manifest row for business operationId: {operation_id}"
            ));
        }
    }

    for (method, path, guard) in &business_routes {
        let key = (method.to_string(), path.to_string());
        if !covered_routes.contains(&key) {
            errors.push(format!(
                "missing denial manifest row for business route: {method} {path} (operationId {})",
                guard.operation_id
            ));
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

fn validate_fixture_shape(fixture: &DenialFixture, errors: &mut Vec<String>) {
    if fixture.version == 0 {
        errors.push("denial fixture version must be >= 1".into());
    }
    if fixture.users_per_org < 3 {
        errors.push(format!(
            "denial fixture usersPerOrg must be >= 3; found {}",
            fixture.users_per_org
        ));
    }
    if fixture.orgs.len() < 2 {
        errors.push(format!(
            "denial fixture must declare at least two orgs; found {}",
            fixture.orgs.len()
        ));
    }
    let expected = ["private", "org", "groups"];
    for visibility in expected {
        if !fixture
            .collection_visibilities
            .iter()
            .any(|v| v == visibility)
        {
            errors.push(format!(
                "denial fixture collectionVisibilities missing {visibility}"
            ));
        }
    }
    if fixture.duplicate_names.collection.trim().is_empty()
        || fixture.duplicate_names.document.trim().is_empty()
    {
        errors.push("denial fixture duplicateNames must be non-empty".into());
    }
    if fixture.role_topology.len() < 3 {
        errors.push(format!(
            "denial fixture roleTopology must include owner/admin/member; found {}",
            fixture.role_topology.len()
        ));
    }
    for org in &fixture.orgs {
        if !fixture.indexed_markers.contains_key(&org.key) {
            errors.push(format!(
                "denial fixture indexedMarkers missing entry for org {}",
                org.key
            ));
        }
    }
    if fixture.object_key_template.trim().is_empty() {
        errors.push("denial fixture objectKeyTemplate must be non-empty".into());
    }
    if !fixture.pre_revoke_tokens {
        errors.push("denial fixture preRevokeTokens must be true".into());
    }
}

fn validate_na_evidence_shape(
    na_evidence: &DenialNaEvidence,
    inventory: &GuardInventory,
    errors: &mut Vec<String>,
) {
    let present: BTreeSet<&str> = na_evidence
        .categories
        .iter()
        .map(|c| c.id.as_str())
        .collect();
    for required in REQUIRED_NA_CATEGORIES {
        if !present.contains(required) {
            errors.push(format!("missing N/A evidence category: {required}"));
        }
    }

    let inventory_operation_ids: BTreeSet<&str> = inventory
        .operations
        .iter()
        .map(|op| op.operation_id.as_str())
        .collect();
    let route_paths: Vec<&str> = ROUTE_INVENTORY.iter().map(|(_, path, _)| *path).collect();
    let openapi_yaml =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi/openapi.yaml"))
            .unwrap_or_default();

    for category in &na_evidence.categories {
        match category.proof_kind {
            NaProofKind::SourceScan => {
                let scan = match &category.scan {
                    Some(scan) => scan,
                    None => {
                        errors.push(format!(
                            "N/A category {} with proofKind=source_scan must declare scan proof",
                            category.id
                        ));
                        continue;
                    }
                };
                for pattern in &scan.absent_route_path_patterns {
                    if route_paths.iter().any(|path| path.contains(pattern))
                        || openapi_yaml.contains(pattern)
                    {
                        errors.push(format!(
                            "N/A category {} source_scan proof stale: route pattern {pattern} now present",
                            category.id
                        ));
                    }
                }
                for pattern in &scan.absent_operation_id_patterns {
                    if inventory_operation_ids
                        .iter()
                        .any(|op| op.contains(pattern.as_str()))
                    {
                        errors.push(format!(
                            "N/A category {} source_scan proof stale: operationId pattern {pattern} now present",
                            category.id
                        ));
                    }
                }
            }
            NaProofKind::CapabilitySubstitution => {
                let substitution = match &category.substitution {
                    Some(sub) => sub,
                    None => {
                        errors.push(format!(
                            "N/A category {} with proofKind=capability_substitution must declare substitution proof",
                            category.id
                        ));
                        continue;
                    }
                };
                for pattern in &substitution.absent_route_path_patterns {
                    if route_paths.iter().any(|path| path.contains(pattern)) {
                        errors.push(format!(
                            "N/A category {} substitution proof stale: signed-url pattern {pattern} now present",
                            category.id
                        ));
                    }
                }
                let registered =
                    registered_test_functions(&substitution.binary).unwrap_or_default();
                for test_name in &substitution.test_names {
                    if !registered.contains(test_name) {
                        errors.push(format!(
                            "N/A category {} substitution references unknown test: {}::{}",
                            category.id, substitution.binary, test_name
                        ));
                    }
                }
            }
        }

        for operation_id in &category.operation_ids {
            if category.id == NA_CATEGORY_SIGNED_URL_CAPABILITY_SUBSTITUTION {
                continue;
            }
            if !inventory_operation_ids.contains(operation_id.as_str()) {
                errors.push(format!(
                    "N/A category {} references unknown operationId: {operation_id}",
                    category.id
                ));
            }
        }
    }

    // N/A rows become invalid when a previously-absent runtime operation appears.
    for category in &na_evidence.categories {
        if category.id == NA_CATEGORY_EXPORT_ROUTE_ABSENT
            && inventory_operation_ids
                .iter()
                .any(|op| op.contains("export"))
        {
            errors.push(
                "N/A evidence stale: export runtime operation now exists in guard inventory".into(),
            );
        }
        if category.id == NA_CATEGORY_AUTOCOMPLETE_ROUTE_ABSENT
            && inventory_operation_ids
                .iter()
                .any(|op| op.contains("autocomplete"))
        {
            errors.push(
                "N/A evidence stale: autocomplete runtime operation now exists in guard inventory"
                    .into(),
            );
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn extract_rust_test_function_names_finds_sync_and_async() {
        let source = r#"
            #[tokio::test]
            async fn live_http_cross_tenant() {}
            #[test]
            fn required_mode_panics_without_db() {}
        "#;
        let names = extract_rust_test_function_names(source);
        assert!(names.contains("live_http_cross_tenant"));
        assert!(names.contains("required_mode_panics_without_db"));
    }

    #[test]
    fn business_guard_operations_exclude_public_probes() {
        let inventory = load_guard_inventory().expect("guard inventory");
        let business: BTreeSet<String> = business_guard_operations(&inventory)
            .into_iter()
            .map(|op| op.operation_id.clone())
            .collect();
        assert!(!business.contains("healthLive"));
        assert!(!business.contains("authLogin"));
        assert!(business.contains("getCollection"));
        assert!(business.contains("redeemDownload"));
    }
}
