//! Hermetic validator for the Phase 1C connected multi-org denial manifest.

mod common;

use common::multi_org_denial::{
    load_denial_fixture, load_denial_manifest, load_denial_na_evidence, validate_denial_manifest,
};
use fileconv_server::auth::guard_inventory::load_guard_inventory;

/// Normative completeness assertion — must pass only when fixture, manifest, and N/A
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

/// Diagnostic pin: while manifest/fixture are incomplete, validator enumerates every
/// missing join deterministically (sorted, no bare counts).
#[test]
fn denial_manifest_validation_diagnostics_enumerate_missing_joins_in_red() {
    let inventory = load_guard_inventory().expect("guard inventory must load");
    let manifest = load_denial_manifest().expect("denial manifest must parse");
    let na_evidence = load_denial_na_evidence().expect("denial N/A evidence must parse");
    let fixture = load_denial_fixture().expect("denial fixture must parse");

    let errors = validate_denial_manifest(&inventory, &manifest, &na_evidence, &fixture)
        .expect_err("RED: incomplete manifest must enumerate missing joins");

    assert!(
        errors
            .iter()
            .any(|err| err.starts_with("missing denial manifest row for business operationId: ")),
        "expected missing business operationId enumeration, got:\n{}",
        errors.join("\n")
    );
    assert!(
        errors
            .iter()
            .any(|err| err.starts_with("missing denial manifest row for business route: ")),
        "expected missing business route enumeration, got:\n{}",
        errors.join("\n")
    );
    assert!(
        errors
            .iter()
            .any(|err| err.contains("denial fixture must declare at least two orgs")),
        "expected fixture incompleteness diagnostic, got:\n{}",
        errors.join("\n")
    );
    assert!(
        !errors
            .iter()
            .any(|err| err.contains("unknown test source") && err.contains("api_http_contracts")),
        "referenced cross-org test must resolve in source, got:\n{}",
        errors.join("\n")
    );
    assert!(
        !errors.iter().any(|err| {
            err.starts_with("missing N/A evidence category: ")
                || err.starts_with("N/A category ") && err.contains("must declare")
        }),
        "N/A evidence scaffold must be structurally valid, got:\n{}",
        errors.join("\n")
    );

    for window in errors.windows(2) {
        assert!(
            window[0] <= window[1],
            "validator diagnostics must be sorted deterministically:\n{}",
            errors.join("\n")
        );
    }
}

#[test]
fn denial_manifest_reports_fixture_incompleteness_in_red() {
    let fixture = load_denial_fixture().expect("denial fixture must parse");
    assert!(
        fixture.orgs.is_empty(),
        "RED fixture intentionally has zero org rows until GREEN"
    );
}
