//! Hermetic validator for the Phase 1C connected multi-org denial manifest.

mod common;

use std::collections::BTreeSet;

use common::multi_org_denial::{
    business_guard_operations, load_denial_fixture, load_denial_manifest, load_denial_na_evidence,
    validate_denial_manifest, DenialRowStatus, REQUIRED_DEFERRED_GUARD_REFS,
    REQUIRED_NA_CATEGORIES,
};
use fileconv_server::auth::guard_inventory::load_guard_inventory;

/// Normative completeness assertion — passes only when fixture, manifest, and N/A
/// evidence fully join guard inventory, business routes, and registered test sources.
#[test]
fn denial_manifest_joins_guard_inventory_routes_and_test_sources() {
    let inventory = load_guard_inventory().expect("guard inventory must load");
    let manifest = load_denial_manifest().expect("denial manifest must parse");
    let na_evidence = load_denial_na_evidence().expect("denial N/A evidence must parse");
    let fixture = load_denial_fixture().expect("denial fixture must parse");

    validate_denial_manifest(&inventory, &manifest, &na_evidence, &fixture).unwrap_or_else(
        |errors| {
            panic!(
                "denial manifest must join guard inventory, routes, and test sources with zero validation errors:\n{}",
                errors.join("\n")
            );
        },
    );
}

#[test]
fn denial_manifest_inventory_counts_are_complete() {
    let inventory = load_guard_inventory().expect("guard inventory");
    let manifest = load_denial_manifest().expect("manifest");
    let business_ops: BTreeSet<_> = business_guard_operations(&inventory)
        .into_iter()
        .map(|op| op.operation_id.as_str())
        .collect();
    let covered: BTreeSet<_> = manifest
        .rows
        .iter()
        .filter(|row| matches!(row.status, DenialRowStatus::Executable))
        .filter_map(|row| row.operation_id.as_deref())
        .collect();
    assert_eq!(covered, business_ops, "every business operationId covered");

    let na_rows: BTreeSet<_> = manifest
        .rows
        .iter()
        .filter(|row| matches!(row.status, DenialRowStatus::Na))
        .filter_map(|row| row.na_category.as_deref())
        .collect();
    assert_eq!(
        na_rows,
        REQUIRED_NA_CATEGORIES
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
    );

    let deferred_rows: BTreeSet<_> = manifest
        .rows
        .iter()
        .filter(|row| matches!(row.status, DenialRowStatus::Deferred))
        .map(|row| row.guard_inventory_ref.as_str())
        .collect();
    assert_eq!(
        deferred_rows,
        REQUIRED_DEFERRED_GUARD_REFS
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
    );

    let primary_http: BTreeSet<_> = manifest
        .rows
        .iter()
        .filter(|row| matches!(row.status, DenialRowStatus::Executable))
        .filter(|row| matches!(row.layer, common::multi_org_denial::DenialLayer::Http))
        .filter(|row| {
            !matches!(
                row.evidence_role,
                Some(common::multi_org_denial::DenialEvidenceRole::Secondary)
            )
        })
        .filter_map(|row| row.operation_id.as_deref())
        .collect();
    let primary_sse: BTreeSet<_> = manifest
        .rows
        .iter()
        .filter(|row| matches!(row.status, DenialRowStatus::Executable))
        .filter(|row| matches!(row.layer, common::multi_org_denial::DenialLayer::Sse))
        .filter_map(|row| row.operation_id.as_deref())
        .collect();
    assert_eq!(
        primary_http.len() + primary_sse.len(),
        business_ops.len(),
        "exactly one primary row per business operation across HTTP/SSE layers"
    );
    assert_eq!(primary_http.len(), 52);
    assert_eq!(primary_sse.len(), 1);
}
