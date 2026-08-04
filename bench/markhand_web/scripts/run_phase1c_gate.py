#!/usr/bin/env python3
"""Phase 1C G1C-SEC deployed qualification harness.

Produces sanitized ``phase-1c-gate.json`` and per-gate evidence under
``bench/markhand_web/reports/phase-1c-gate/``. Default status is honest
``not_run``. ``pass`` requires ``MARKHAND_PHASE1C_GATE=1``, live deployed
probes, registry/report contract validation, and redaction-safe evidence.

Task 16 Commit A: validator/harness contract stub — live probes not implemented.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
MARKHAND_ROOT = ROOT / "bench/markhand_web"
DEFAULT_REPORT = MARKHAND_ROOT / "reports/phase-1c-gate/phase-1c-gate.json"
TEMPLATE_REPORT = MARKHAND_ROOT / "reports/phase-1c-gate/phase-1c-gate.template.json"
IMAGES_LOCK = ROOT / "deploy/poc/images.lock.json"
DENIAL_MANIFEST = ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
G1C_COMMAND = "python3 bench/markhand_web/scripts/run_phase1c_gate.py"

# P1C.8 item coverage required across gate evidence (design §P1C.8).
P1C8_REQUIRED_ITEMS: frozenset[str] = frozenset(
    {
        "noisy_neighbor_fairness",
        "token_rotation_reuse_revoke",
        "acl_cache_invalidation",
        "qdrant_partial_fail_closed",
        "reconcile_scope_bound",
        "container_vulnerability_scan",
        "cross_tenant_leakage",
        "membership_acl_revoke_bound",
        "quota_recovery_after_failure",
        "admin_mutation_audit_coverage",
        "worker_runtime_role_proof",
    }
)

GATE_TO_P1C8: dict[str, tuple[str, ...]] = {
    "G1C-SEC-LEAKAGE": ("cross_tenant_leakage",),
    "G1C-SEC-REVOKE": ("membership_acl_revoke_bound",),
    "G1C-SEC-ACL-CACHE": ("acl_cache_invalidation",),
    "G1C-SEC-QUOTA-RECOVERY": ("quota_recovery_after_failure", "reconcile_scope_bound"),
    "G1C-SEC-NOISY-NEIGHBOR": ("noisy_neighbor_fairness",),
    "G1C-SEC-AUDIT-COVERAGE": ("admin_mutation_audit_coverage",),
    "G1C-SEC-WORKER-ROLE": ("worker_runtime_role_proof",),
    "G1C-SEC-CONTAINER-VULNS": ("container_vulnerability_scan",),
    "G1C-SEC-STALE-TOKENS": ("token_rotation_reuse_revoke",),
    "G1C-SEC-QDRANT-FAIL-CLOSED": ("qdrant_partial_fail_closed",),
}


def _load_gate_validator():
    spec = importlib.util.spec_from_file_location(
        "check_markhand_gates", ROOT / "scripts/check-markhand-gates.py"
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load scripts/check-markhand-gates.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GATES = _load_gate_validator()


class HarnessWriteError(RuntimeError):
    """Raised when report/evidence write fails redaction or atomic commit."""


def atomic_write_report(
    path: Path,
    report: dict[str, Any],
    *,
    repo_root: Path | None = None,
) -> None:
    """Commit A stub — atomic fail-closed writes not implemented."""
    raise HarnessWriteError("atomic_write_not_implemented")


def evaluate_report(
    report: dict[str, Any],
    *,
    repo_root: Path | None = None,
    markhand_root: Path | None = None,
    bind_current_git: bool = False,
    evidence_must_exist: bool = True,
) -> tuple[str, list[str]]:
    """Return (status, blockers). Only ``pass`` when every acceptance gate holds."""
    blockers: list[str] = ["harness_not_implemented"]
    workspace = repo_root or ROOT
    markhand = markhand_root or MARKHAND_ROOT

    if not isinstance(report, dict):
        return "fail", ["report_must_be_object"]

    status = report.get("status")
    if status not in {"pass", "fail", "incomplete", "not_run"}:
        blockers.append("status_type")

    # Commit A stub: defer to registry validator only; harness checks missing.
    try:
        registry = GATES.load_json_yaml(markhand / "gates.yaml")
    except (OSError, ValueError) as error:
        blockers.append(f"registry_load:{error}")
        registry = {"gates": []}

    template_mode = status != "pass"
    errors = GATES.phase1c_gate_report_errors(
        report,
        registry=registry,
        root=markhand,
        repo_root=workspace,
        workspace_root=workspace,
        template_mode=template_mode,
    )
    blockers.extend(errors)

    if status == "pass":
        return "fail", blockers
    if status == "not_run":
        return "not_run", blockers
    return "fail", blockers


def validate_report_cli(path: Path) -> int:
    report = json.loads(path.read_text(encoding="utf-8"))
    status, blockers = evaluate_report(report, bind_current_git=True)
    print(json.dumps({"status": status, "blockers": blockers}, indent=2, sort_keys=True))
    return 0 if status == "pass" else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="Phase 1C G1C-SEC qualification harness")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--validate-report",
        type=Path,
        default=None,
        help="Validate phase-1c-gate.json and print {status,blockers}",
    )
    args = parser.parse_args()
    if args.self_test:
        return 0
    if args.validate_report is not None:
        return validate_report_cli(args.validate_report.resolve())
    if os.environ.get("MARKHAND_PHASE1C_GATE") != "1":
        print(DEFAULT_REPORT)
        return 0
    print("Phase 1C harness live run not implemented", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
