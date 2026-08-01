#!/usr/bin/env python3
"""Generate crates/server/tests/fixtures/multi-org-denial.manifest.json (Task 12 GREEN)."""

from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
GUARD = json.loads((ROOT / "crates/server/openapi/guard-inventory.json").read_text())

PUBLIC = {op["operationId"] for op in GUARD["operations"] if op["authzKind"] == "public"}

# (binary, testName, layer) per operationId — semantically aligned cross-org / denial evidence.
EXECUTABLE: dict[str, tuple[str, str, str]] = {
    "authMe": (
        "multi_org_denial",
        "shared_world_http_surfaces_respect_org_scope",
        "http",
    ),
    "createUpload": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "listCollections": (
        "multi_org_denial",
        "shared_world_http_surfaces_respect_org_scope",
        "http",
    ),
    "createCollection": (
        "multi_org_denial",
        "shared_world_http_surfaces_respect_org_scope",
        "http",
    ),
    "getCollection": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "updateCollection": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "deleteCollection": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "assignCollectionProject": (
        "multi_org_denial",
        "shared_world_http_surfaces_respect_org_scope",
        "http",
    ),
    "listDocuments": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "approveIntake": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "getDocument": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "deleteDocument": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "previewDocument": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "listDocumentVersions": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "getDocumentVersion": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "diffDocumentVersions": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "publishDocumentVersion": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "issueDownloadCapability": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "redeemDownload": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "reindexDocument": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "resolveCitation": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "listConflicts": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "getConflict": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "getConflictEvidence": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "triageConflict": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "getJob": (
        "api_http_contracts",
        "live_http_unauthenticated_and_cross_tenant_are_consistent",
        "http",
    ),
    "jobEvents": (
        "sse_stream_readiness",
        "live_job_sse_replay_worker_restart_and_cross_org_idor",
        "sse",
    ),
    "search": (
        "api_http_contracts",
        "live_http_retrieval_refuses_foreign_collection_scope",
        "http",
    ),
    "ask": (
        "api_http_contracts",
        "live_http_retrieval_refuses_foreign_collection_scope",
        "http",
    ),
    "askStream": (
        "api_http_contracts",
        "live_http_retrieval_refuses_foreign_collection_scope",
        "http",
    ),
    "listOrgs": ("orgs", "list_orgs_shows_only_the_callers_own_orgs", "http"),
    "createOrg": ("orgs", "create_org_succeeds_and_caller_becomes_owner", "http"),
    "switchOrg": (
        "orgs",
        "switch_denies_and_audits_a_real_org_the_caller_is_not_a_member_of",
        "http",
    ),
    "getOrg": (
        "orgs",
        "get_org_detail_is_identical_for_nonexistent_and_not_a_member",
        "http",
    ),
    "listProjects": ("projects", "org_b_cannot_see_or_rename_org_as_project", "http"),
    "createProject": ("projects", "org_b_cannot_see_or_rename_org_as_project", "http"),
    "updateProject": ("projects", "org_b_cannot_see_or_rename_org_as_project", "http"),
    "listMembers": ("members", "cross_org_denial_covers_every_member_endpoint", "http"),
    "listMemberInvites": (
        "multi_org_denial",
        "shared_world_http_surfaces_respect_org_scope",
        "http",
    ),
    "createMemberInvite": (
        "members",
        "cross_org_denial_covers_every_member_endpoint",
        "http",
    ),
    "acceptMemberInvite": (
        "members",
        "cross_org_denial_covers_every_member_endpoint",
        "http",
    ),
    "revokeMemberInvite": (
        "members",
        "cross_org_denial_covers_every_member_endpoint",
        "http",
    ),
    "patchMember": ("members", "cross_org_denial_covers_every_member_endpoint", "http"),
    "deleteMember": ("members", "cross_org_denial_covers_every_member_endpoint", "http"),
    "getUsage": ("members", "cross_org_denial_covers_every_member_endpoint", "http"),
    "listAudit": ("audit_read", "list_audit_never_leaks_across_orgs", "http"),
    "getGraph": ("graph", "graph_org_isolation_org_b_does_not_see_org_a_nodes", "http"),
    "listChatSessions": ("chat_history", "org_b_cannot_see_or_open_org_a_session", "http"),
    "createChatSession": ("chat_history", "org_b_cannot_see_or_open_org_a_session", "http"),
    "getChatSession": ("chat_history", "org_b_cannot_see_or_open_org_a_session", "http"),
    "updateChatSession": (
        "chat_history",
        "user_b_cannot_see_open_or_delete_user_a_session_same_org",
        "http",
    ),
    "deleteChatSession": (
        "chat_history",
        "user_b_cannot_see_open_or_delete_user_a_session_same_org",
        "http",
    ),
    "appendChatTurn": (
        "chat_history",
        "user_b_cannot_see_open_or_delete_user_a_session_same_org",
        "http",
    ),
}

# Additional layer-specific evidence (unique manifest ids; may repeat operationId).
EXTRA_ROWS: list[tuple[str, str, str, str, str, str | None]] = [
    (
        "deleteDocument",
        "direct_service_authz",
        "doc_delete_permission_required_at_deletion_service",
        "service",
        "denial-deleteDocument-service",
        None,
    ),
    (
        "publishDocumentVersion",
        "direct_service_authz",
        "doc_publish_permission_required_at_direct_db_publish",
        "service",
        "denial-publishDocumentVersion-service",
        None,
    ),
    (
        "patchMember",
        "direct_service_authz",
        "member_manage_permission_required_at_direct_service_patch_and_delete",
        "service",
        "denial-patchMember-service",
        None,
    ),
    (
        "deleteMember",
        "direct_service_authz",
        "member_manage_permission_required_at_direct_service_patch_and_delete",
        "service",
        "denial-deleteMember-service",
        None,
    ),
    (
        "listAudit",
        "direct_service_authz",
        "audit_view_permission_required_at_direct_list_page",
        "service",
        "denial-listAudit-service",
        None,
    ),
    (
        "getJob",
        "direct_service_authz",
        "jobs_system_permission_required_for_documentless_job_access",
        "service",
        "denial-getJob-service",
        None,
    ),
    (
        "search",
        "repositories",
        "cross_org_deny_via_predicate_and_rls",
        "repository",
        "denial-search-repository",
        None,
    ),
    (
        "ask",
        "repositories",
        "cross_org_deny_via_predicate_and_rls",
        "repository",
        "denial-ask-repository",
        None,
    ),
    (
        "reindexDocument",
        "jobs",
        "org_isolation_prevents_cross_org_claim_see_and_mutate",
        "worker",
        "denial-reindexDocument-worker",
        None,
    ),
    (
        "getJob",
        "jobs",
        "org_isolation_prevents_cross_org_claim_see_and_mutate",
        "worker",
        "denial-getJob-worker",
        None,
    ),
    (
        "search",
        "storage",
        "cross_org_point_overwrite_rejected",
        "storage",
        "denial-search-storage",
        None,
    ),
    (
        "createUpload",
        "storage",
        "cross_org_object_key_operation_rejected",
        "storage",
        "denial-createUpload-storage",
        None,
    ),
    (
        "switchOrg",
        "acl_cache",
        "cached_context_denies_immediately_after_role_downgrade",
        "cache",
        "denial-switchOrg-cache",
        None,
    ),
    (
        "resolveCitation",
        "citation_authz_matrix",
        "live_citation_authz_expiry_replay_idor_and_immediate_deny",
        "http",
        "denial-resolveCitation-citation",
        "secondary",
    ),
    (
        "previewDocument",
        "citation_authz_matrix",
        "live_citation_authz_expiry_replay_idor_and_immediate_deny",
        "http",
        "denial-previewDocument-citation",
        "secondary",
    ),
]

# Task 13 deferred cross-org denial gaps — explicit incomplete coverage (not executable).
DEFERRED_GAPS: list[dict] = [
    {
        "id": "deferred-task13-indexed-fts-ask",
        "guardInventoryRef": "task13:indexed_fts_ask",
        "layer": "http",
        "relatedOperationIds": ["search", "ask", "askStream"],
        "coverageNote": "Indexed FTS/Q&A must not return foreign marker strings after full index artifacts exist",
    },
    {
        "id": "deferred-task13-duplicate-names",
        "guardInventoryRef": "task13:duplicate_names",
        "layer": "http",
        "relatedOperationIds": ["listCollections", "getCollection", "listDocuments", "getDocument"],
        "coverageNote": "Same display names across orgs must not create existence oracles",
    },
    {
        "id": "deferred-task13-org-switch-cache",
        "guardInventoryRef": "task13:org_switch_cache",
        "layer": "cache",
        "relatedOperationIds": ["switchOrg"],
        "coverageNote": "Org switch must not reuse previous org ACL/cache scope",
    },
    {
        "id": "deferred-task13-stale-tokens",
        "guardInventoryRef": "task13:stale_tokens",
        "layer": "http",
        "relatedOperationIds": ["authMe"],
        "coverageNote": "Pre-revoke tokens must fail after downgrade, suspend, and remove",
    },
    {
        "id": "deferred-task13-preview-download-sse",
        "guardInventoryRef": "task13:preview_download_sse",
        "layer": "http",
        "relatedOperationIds": [
            "previewDocument",
            "issueDownloadCapability",
            "redeemDownload",
            "getJob",
            "jobEvents",
        ],
        "coverageNote": "Preview, download, job, and SSE surfaces must hide foreign IDs",
    },
    {
        "id": "deferred-task13-in-flight-ask-revoke",
        "guardInventoryRef": "task13:in_flight_ask_revoke",
        "layer": "sse",
        "relatedOperationIds": ["askStream"],
        "coverageNote": "In-flight ask must emit no foreign content after ACL revoke",
    },
]

NA_ROWS = [
    {
        "id": "na-export-route-absent",
        "guardInventoryRef": "export_route_absent",
        "layer": "http",
        "status": "na",
        "naCategory": "export_route_absent",
    },
    {
        "id": "na-autocomplete-route-absent",
        "guardInventoryRef": "autocomplete_route_absent",
        "layer": "http",
        "status": "na",
        "naCategory": "autocomplete_route_absent",
    },
    {
        "id": "na-signed-url-capability-substitution",
        "guardInventoryRef": "signed_url_capability_substitution",
        "layer": "http",
        "status": "na",
        "naCategory": "signed_url_capability_substitution",
    },
    {
        "id": "na-reserved-permission-no-runtime",
        "guardInventoryRef": "reserved_permission_no_runtime",
        "layer": "http",
        "status": "na",
        "naCategory": "reserved_permission_no_runtime",
    },
    {
        "id": "na-embedding-token-metering-local-mock",
        "guardInventoryRef": "embedding_token_metering_local_mock",
        "layer": "http",
        "status": "na",
        "naCategory": "embedding_token_metering_local_mock",
    },
]


def main() -> None:
    rows: list[dict] = []
    business = [
        op["operationId"]
        for op in GUARD["operations"]
        if op["operationId"] not in PUBLIC
    ]
    missing = [op for op in business if op not in EXECUTABLE]
    if missing:
        raise SystemExit(f"missing executable mapping for: {missing}")

    for op_id in sorted(EXECUTABLE):
        binary, test_name, layer = EXECUTABLE[op_id]
        rows.append(
            {
                "id": f"denial-{op_id}",
                "binary": binary,
                "testName": test_name,
                "operationId": op_id,
                "guardInventoryRef": op_id,
                "layer": layer,
                "status": "executable",
                "coverageState": "complete",
                "evidenceRole": "primary",
            }
        )

    for op_id, binary, test_name, layer, row_id, evidence_role in EXTRA_ROWS:
        row = {
            "id": row_id,
            "binary": binary,
            "testName": test_name,
            "operationId": op_id,
            "guardInventoryRef": op_id,
            "layer": layer,
            "status": "executable",
            "coverageState": "complete",
        }
        if evidence_role:
            row["evidenceRole"] = evidence_role
        rows.append(row)

    for gap in DEFERRED_GAPS:
        rows.append(
            {
                "id": gap["id"],
                "guardInventoryRef": gap["guardInventoryRef"],
                "layer": gap["layer"],
                "status": "deferred",
                "coverageState": "deferred",
                "deferredTask": "task13",
                "coverageNote": gap["coverageNote"],
                "relatedOperationIds": gap["relatedOperationIds"],
            }
        )

    rows.extend(NA_ROWS)
    manifest = {"version": 1, "rows": rows}
    OUT.write_text(json.dumps(manifest, indent=2) + "\n")
    deferred = len(DEFERRED_GAPS)
    extras = len(EXTRA_ROWS)
    print(
        f"wrote {len(rows)} rows "
        f"({len(EXECUTABLE)} primary + {extras} supplemental + {deferred} deferred + {len(NA_ROWS)} na) -> {OUT}"
    )


if __name__ == "__main__":
    main()
