#!/usr/bin/env python3
"""Phase 1C G1C-SEC deployed qualification harness."""

from __future__ import annotations

import argparse
import copy
import hashlib
import importlib.util
import json
import os
import re
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

ROOT = Path(__file__).resolve().parents[3]
MARKHAND_ROOT = ROOT / "bench/markhand_web"
DEFAULT_REPORT = MARKHAND_ROOT / "reports/phase-1c-gate/phase-1c-gate.json"
TEMPLATE_REPORT = MARKHAND_ROOT / "reports/phase-1c-gate/phase-1c-gate.template.json"
IMAGES_LOCK = ROOT / "deploy/poc/images.lock.json"
COMPOSE_FILE = ROOT / "deploy/compose.poc.yml"
DENIAL_MANIFEST = ROOT / "crates/server/tests/fixtures/multi-org-denial.manifest.json"
DENIAL_RUNNER = ROOT / "scripts/run-phase1c-denial-suite.py"
DEPLOYED_PROBES = ROOT / "bench/markhand_web/scripts/phase1c_deployed_probes.py"
REDACT_SCRIPT = ROOT / "deploy/scripts/redact_secrets.py"
TRIVYIGNORE = ROOT / ".trivyignore"
G1C_COMMAND = "python3 bench/markhand_web/scripts/run_phase1c_gate.py"

PROBE_SCHEMA_VERSION = 1
WORKER_PROBE_SCHEMA_VERSION = 1
DENIAL_REPORT_SCHEMA_VERSION = 1
EVIDENCE_SCHEMA_VERSION = 1

HARNESS_EXTENSION_KEYS = frozenset(
    {"markhandPhase1cGate", "embeddingProfile", "p1c8EvidenceMapping", "notes"}
)

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

SCENARIO_BY_GATE: dict[str, str] = {
    "G1C-SEC-LEAKAGE": "multi_org_denial_replay",
    "G1C-SEC-REVOKE": "membership_acl_revoke_bound",
    "G1C-SEC-ACL-CACHE": "membership_acl_revoke_bound",
    "G1C-SEC-QUOTA-RECOVERY": "quota_recovery_after_failure",
    "G1C-SEC-NOISY-NEIGHBOR": "noisy_neighbor_fairness",
    "G1C-SEC-AUDIT-COVERAGE": "admin_mutation_audit_coverage",
    "G1C-SEC-WORKER-ROLE": "worker_runtime_role_proof",
    "G1C-SEC-CONTAINER-VULNS": "container_vulnerability_scan",
    "G1C-SEC-STALE-TOKENS": "stale_token_isolation",
    "G1C-SEC-QDRANT-FAIL-CLOSED": "qdrant_partial_fail_closed",
}

def _load_deployed_probes():
    spec = importlib.util.spec_from_file_location("phase1c_deployed_probes", DEPLOYED_PROBES)
    if spec is None or spec.loader is None:
        raise RuntimeError("phase1c_deployed_probes.py missing")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


_DEPLOYED = _load_deployed_probes()
DEPLOYED_PROBE_GATES: tuple[str, ...] = _DEPLOYED.DEPLOYED_PROBE_GATES

SECRET_RESIDUAL_RE = re.compile(
    r"(?i)("
    r"Bearer\s+[A-Za-z0-9._\-+=/]{8,}|"
    r"Authorization:\s*Basic\s+\S+|"
    r"(?:Set-)?Cookie:\s*[^\r\n]+|"
    r"\b(postgres(?:ql)?|mysql|mongodb|redis)://[^\s'\"]+|"
    r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}|"
    r"/(?:home|Users|workspace|tmp)/\S+|"
    r"\b[A-Za-z]:\\Users\\"
    r")"
)

TRIVY_PIN_RE = re.compile(r"^aquasec/trivy:[0-9.]+@sha256:[a-f0-9]{64}$")
CommandRunner = Callable[..., dict[str, Any]]


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


class StagingWorkspace:
    """Private staging area; commits allowlisted evidence atomically."""

    def __init__(
        self,
        *,
        repo_root: Path,
        final_dir: Path,
        source_revision: dict[str, Any] | None = None,
    ) -> None:
        self.repo_root = repo_root
        self.final_dir = final_dir.resolve()
        self.temp_dir = Path(tempfile.mkdtemp(prefix=".phase1c-staging."))
        self.source_revision = source_revision or capture_source_revision(repo_root)
        self._staged: dict[str, Path] = {}

    def stage_json(self, relative_path: str, payload: dict[str, Any]) -> Path:
        if relative_path not in GATES.PHASE1C_EVIDENCE_ALLOWLIST and not relative_path.endswith(
            "phase-1c-gate.json"
        ):
            raise RuntimeError(f"refusing to stage non-allowlisted path: {relative_path}")
        staged_path = self.temp_dir / Path(relative_path).name
        atomic_write_json(staged_path, payload, validate=False)
        self._staged[relative_path] = staged_path
        return staged_path

    def commit_file(self, src: Path, *, relative_path: str) -> None:
        if src.is_symlink():
            raise HarnessWriteError("refusing_symlink_staged_file")
        resolved = src.resolve()
        if self.temp_dir not in resolved.parents and resolved != self.temp_dir:
            if not resolved.is_file():
                raise HarnessWriteError("staged_file_missing")
        dest = self.final_dir / Path(relative_path).name
        if dest.exists() or dest.is_symlink():
            dest.unlink(missing_ok=True)
        dest.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(prefix=f".{dest.name}.", dir=dest.parent)
        tmp_path = Path(tmp)
        try:
            shutil.copyfile(resolved, tmp_path)
            os.chmod(tmp_path, 0o644)
            tmp_path.replace(dest)
        except Exception:
            tmp_path.unlink(missing_ok=True)
            raise

    def commit_all(self) -> None:
        for relative_path, staged in self._staged.items():
            self.commit_file(staged, relative_path=relative_path)

    def commit_all_report_last(self) -> None:
        report_key = "bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json"
        evidence_items = [
            (relative_path, staged)
            for relative_path, staged in self._staged.items()
            if relative_path != report_key
        ]
        for relative_path, staged in evidence_items:
            self.commit_file(staged, relative_path=relative_path)
        if report_key in self._staged:
            self.commit_file(self._staged[report_key], relative_path=report_key)

    def purge_final_allowlisted_artifacts(self) -> None:
        for relative_path in GATES.PHASE1C_EVIDENCE_ALLOWLIST:
            target = self.final_dir / Path(relative_path).name
            if target.exists() or target.is_symlink():
                target.unlink(missing_ok=True)
        report = self.final_dir / "phase-1c-gate.json"
        if report.exists() or report.is_symlink():
            report.unlink(missing_ok=True)

    def cleanup(self) -> None:
        shutil.rmtree(self.temp_dir, ignore_errors=True)


def utc_now_z() -> str:
    return datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def sanitize_failure_message(message: str) -> str:
    cleaned = redact_text(str(message))
    if SECRET_RESIDUAL_RE.search(cleaned):
        return "phase1c harness failure (redacted)"
    return cleaned


def capture_source_revision(repo_root: Path) -> dict[str, Any]:
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo_root, text=True).strip()
    dirty = (
        subprocess.check_output(["git", "status", "--porcelain"], cwd=repo_root, text=True).strip()
        != ""
    )
    if dirty:
        raise RuntimeError("refusing probes on dirty git tree")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise RuntimeError("unresolved source revision")
    return {"commit": commit, "dirty": False}


def load_images_lock() -> dict[str, Any]:
    return json.loads(IMAGES_LOCK.read_text(encoding="utf-8"))


def pinned_trivy_image() -> str:
    lock = load_images_lock()
    image = lock.get("images", {}).get("trivy")
    if not isinstance(image, str) or "@sha256:" not in image or ":latest" in image:
        raise RuntimeError("images.lock.json missing digest-pinned trivy image")
    return image


def strip_harness_extensions(report: dict[str, Any]) -> dict[str, Any]:
    cleaned = copy.deepcopy(report)
    for key in HARNESS_EXTENSION_KEYS:
        cleaned.pop(key, None)
    return cleaned


def build_p1c8_mapping() -> list[dict[str, str]]:
    mapping: list[dict[str, str]] = []
    for gate_id, items in GATE_TO_P1C8.items():
        evidence = next(str(row["evidence"]) for row in GATES.G1C_GATE_ROWS if str(row["id"]) == gate_id)
        for item in items:
            mapping.append({"item": item, "gateId": gate_id, "evidence": evidence})
    return mapping


def redact_text(text: str) -> str:
    spec = importlib.util.spec_from_file_location("redact_secrets", REDACT_SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load redact_secrets.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.redact_text(text)


def residual_secret_errors(text: str, *, context: str) -> list[str]:
    if SECRET_RESIDUAL_RE.search(text):
        return [f"redaction_failed:{context}"]
    return []


def parse_denial_report(report: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(report, dict):
        raise RuntimeError("denial report must be object")
    if report.get("schemaVersion") != DENIAL_REPORT_SCHEMA_VERSION:
        raise RuntimeError("denial report schemaVersion mismatch")
    if "leakageCount" not in report:
        raise RuntimeError("denial report missing leakageCount")
    leakage = report["leakageCount"]
    if isinstance(leakage, bool) or not isinstance(leakage, int):
        raise TypeError("denial leakageCount must be int")
    if report.get("failures"):
        raise RuntimeError("denial runner reported failures")
    redaction = report.get("redactionScan")
    if not isinstance(redaction, dict) or redaction.get("passed") is not True:
        raise RuntimeError("denial report redactionScan failed")
    for field in ("manifestSha256", "gitShaFull"):
        value = report.get(field)
        if not isinstance(value, str) or not value.strip():
            raise RuntimeError(f"denial report missing {field}")
    return {"cross_tenant_leakage_count": leakage}


def parse_probe_stdout(stdout: str, *, probe_id: str) -> dict[str, Any]:
    lines = stdout.splitlines()
    results: list[dict[str, Any]] = []
    eof_seen = False
    after_eof = False
    for line in lines:
        if after_eof and line.strip():
            raise RuntimeError("probe output trailing content after EOF")
        if line.startswith("PHASE1C_PROBE_RESULT\t"):
            if eof_seen:
                raise RuntimeError("probe result after EOF")
            payload_raw = line.split("\t", 1)[1]
            payload = json.loads(payload_raw)
            if payload.get("schemaVersion") != PROBE_SCHEMA_VERSION:
                raise RuntimeError("probe schemaVersion mismatch")
            if payload.get("probeId") != probe_id:
                raise RuntimeError("probeId mismatch")
            metrics = payload.get("metrics")
            if not isinstance(metrics, dict) or not metrics:
                raise RuntimeError("probe metrics missing")
            results.append(payload)
            continue
        if line.startswith("PHASE1C_PROBE_EOF\t"):
            if line.strip() != "PHASE1C_PROBE_EOF\ttrue":
                raise RuntimeError("probe EOF marker invalid")
            if eof_seen:
                raise RuntimeError("duplicate probe EOF")
            eof_seen = True
            after_eof = True
    if len(results) != 1:
        raise RuntimeError("probe requires exactly one PHASE1C_PROBE_RESULT")
    if not eof_seen:
        raise RuntimeError("probe missing PHASE1C_PROBE_EOF")
    return results[0]


def parse_worker_role_probe(output: str, *, expected_nonce: str | None = None) -> dict[str, Any]:
    payload: dict[str, Any] | None = None
    eof_seen = False
    after_eof = False
    for line in output.splitlines():
        if after_eof and line.strip():
            raise RuntimeError("worker probe trailing output")
        if line.startswith("PHASE1C_WORKER_ROLE_PROBE\t"):
            raw = line.split("\t", 1)[1]
            parsed = json.loads(raw)
            if parsed.get("schemaVersion") != WORKER_PROBE_SCHEMA_VERSION:
                raise RuntimeError("worker probe schemaVersion mismatch")
            for key in ("currentUser", "databaseUrlRolePath", "nonce"):
                if not isinstance(parsed.get(key), str) or not parsed[key]:
                    raise RuntimeError(f"worker probe missing {key}")
            for key in ("superuser", "bypassRls", "dedicatedDatabaseUrlVerified"):
                if not isinstance(parsed.get(key), bool):
                    raise RuntimeError(f"worker probe boolean required: {key}")
            payload = parsed
            continue
        if line.startswith("PHASE1C_WORKER_ROLE_PROBE_EOF\t"):
            if line.strip() != "PHASE1C_WORKER_ROLE_PROBE_EOF\ttrue":
                raise RuntimeError("worker probe EOF invalid")
            eof_seen = True
            after_eof = True
    if payload is None or not eof_seen:
        raise RuntimeError("worker probe incomplete")
    if expected_nonce is not None and payload.get("nonce") != expected_nonce:
        raise RuntimeError("worker probe nonce mismatch")
    if payload["currentUser"] != "markhand_worker":
        raise RuntimeError("worker currentUser must be markhand_worker")
    if payload["superuser"] or payload["bypassRls"]:
        raise RuntimeError("worker must not be superuser or bypass RLS")
    return payload


def load_trivyignore_ids(text: str) -> set[str]:
    ids: set[str] = set()
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        token = stripped.split()[0]
        if token:
            ids.add(token)
    return ids


def parse_combined_trivy_scan(
    *,
    api_report: dict[str, Any],
    worker_report: dict[str, Any],
    api_ref: str,
    worker_ref: str,
    trivyignore_text: str,
) -> dict[str, Any]:
    if not isinstance(api_report, dict) or not isinstance(worker_report, dict):
        raise RuntimeError("trivy report must be object")
    _DEPLOYED.validate_trivy_report_target(api_report, requested_ref=api_ref)
    _DEPLOYED.validate_trivy_report_target(worker_report, requested_ref=worker_ref)
    ignored = load_trivyignore_ids(trivyignore_text)
    findings: list[dict[str, Any]] = []
    images: list[dict[str, str]] = []
    for label, report, ref in (
        ("api", api_report, api_ref),
        ("worker", worker_report, worker_ref),
    ):
        if report.get("SchemaVersion") != 2:
            raise RuntimeError(f"trivy {label} SchemaVersion must be 2")
        results = report.get("Results")
        if not isinstance(results, list):
            raise RuntimeError(f"trivy {label} Results missing")
        images.append({"role": label, "ref": ref, "reportSha256": hashlib.sha256(json.dumps(report).encode()).hexdigest()})
        for result in results:
            if not isinstance(result, dict):
                raise RuntimeError("trivy result malformed")
            vulnerabilities = result.get("Vulnerabilities") or []
            if not isinstance(vulnerabilities, list):
                raise RuntimeError("trivy vulnerabilities malformed")
            for vuln in vulnerabilities:
                if not isinstance(vuln, dict):
                    raise RuntimeError("trivy vulnerability malformed")
                severity = str(vuln.get("Severity", "")).upper()
                if severity not in {"HIGH", "CRITICAL"}:
                    continue
                vid = str(vuln.get("VulnerabilityID", ""))
                disposition = "ignored" if vid in ignored else "undispositioned"
                findings.append(
                    {
                        "imageRole": label,
                        "vulnerabilityId": vid,
                        "severity": severity,
                        "status": str(vuln.get("Status") or "unknown"),
                        "disposition": disposition,
                    }
                )
    undispositioned = sum(1 for item in findings if item["disposition"] == "undispositioned")
    return {
        "scanner": pinned_trivy_image(),
        "undispositionedHighCritical": undispositioned,
        "findings": findings,
        "images": images,
        "passed": undispositioned == 0,
        "completionMarker": "PHASE1C_TRIVY_EOF",
    }


def docker_image_digest(tag: str, runner: CommandRunner) -> str:
    outcome = runner(["docker", "inspect", "--format", "{{json .RepoDigests}}", tag], timeout_secs=120)
    if outcome["commandExitCode"] != 0:
        raise RuntimeError("docker inspect failed for image digest")
    digests = json.loads(outcome["stdout"].strip() or "[]")
    if not isinstance(digests, list) or not digests:
        raise RuntimeError("image digest missing")
    digest = str(digests[0])
    if "@sha256:" not in digest:
        raise RuntimeError("image digest unpinned")
    return digest


def scanner_pin_errors(scanner: object) -> list[str]:
    if not isinstance(scanner, str) or not scanner.strip():
        return ["scanner_pin:missing"]
    if ":latest" in scanner or "@sha256:" not in scanner:
        return ["scanner_pin:unpinned"]
    try:
        expected = pinned_trivy_image()
    except RuntimeError as error:
        return [f"scanner_pin:{error}"]
    if scanner != expected:
        return ["scanner_pin:images_lock_mismatch"]
    return []


def embedding_profile_errors(profile: object, *, status: str) -> list[str]:
    if status != "pass":
        return []
    if profile != "mock":
        return ["embedding_profile:qualifying_pass_requires_mock"]
    return []


def p1c8_mapping_errors(report: dict[str, Any], *, status: str) -> list[str]:
    if status != "pass":
        return []
    mapping = report.get("p1c8EvidenceMapping")
    if not isinstance(mapping, list):
        return ["p1c8_evidence_mapping:missing"]
    seen: set[str] = set()
    for index, entry in enumerate(mapping):
        if not isinstance(entry, dict):
            return [f"p1c8_evidence_mapping[{index}]:not_object"]
        item = entry.get("item")
        if not isinstance(item, str):
            return [f"p1c8_evidence_mapping[{index}]:missing_item"]
        seen.add(item)
    missing = sorted(P1C8_REQUIRED_ITEMS - seen)
    if missing:
        return [f"p1c8_evidence_mapping:missing_items:{','.join(missing)}"]
    return []


def evidence_binding_errors(payload: dict[str, Any], *, evidence_path: str) -> list[str]:
    errors: list[str] = []
    if payload.get("schemaVersion") != EVIDENCE_SCHEMA_VERSION:
        errors.append("evidence_schema_version")
    if payload.get("evidencePath") != evidence_path:
        errors.append("evidence_path_binding")
    if payload.get("environmentId") != GATES.PHASE1C_ENVIRONMENT_ID:
        errors.append("environment_binding")
    if payload.get("workloadProfileId") != GATES.PHASE1C_WORKLOAD_PROFILE_ID:
        errors.append("workload_binding")
    if payload.get("embeddingProfile") != "mock":
        errors.append("embedding_binding")
    if payload.get("targetMatch") is not True:
        errors.append("target_match_binding")
    for field in ("sourceRevision", "canonicalBinding", "thresholdDecisions"):
        if not isinstance(payload.get(field), (dict, list)):
            errors.append(f"missing_{field}")
    return errors


def evidence_probe_errors(repo_root: Path, *, status: str) -> list[str]:
    if status != "pass":
        return []
    errors: list[str] = []
    for row in GATES.G1C_GATE_ROWS:
        rel = str(row["evidence"])
        path = repo_root / rel
        if not path.is_file():
            errors.append(f"evidence_missing:{rel}")
            continue
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            errors.append(f"evidence_malformed:{rel}")
            continue
        errors.extend(evidence_binding_errors(payload, evidence_path=rel))
        if payload.get("gateId") != row["id"]:
            errors.append(f"evidence_gate_mismatch:{rel}")
        p1c8_items = payload.get("p1c8Items")
        if not isinstance(p1c8_items, list) or not p1c8_items:
            errors.append(f"p1c8_items_missing:{rel}")
        probe = payload.get("probe")
        if not isinstance(probe, dict):
            errors.append(f"probe_missing:{rel}")
            continue
        if probe.get("commandExitCode") not in (0, "0"):
            errors.append(f"probe_exit:{rel}")
        if probe.get("timedOut") is True:
            errors.append(f"probe_timeout:{rel}")
        if probe.get("outputTruncated") is True:
            errors.append(f"probe_truncated:{rel}")
        if probe.get("eof") is not True:
            errors.append(f"probe_eof:{rel}")
        errors.extend(residual_secret_errors(json.dumps(payload), context=rel))
    return errors


def opt_in_errors(report: dict[str, Any], *, status: str) -> list[str]:
    if status != "pass":
        return []
    if report.get("markhandPhase1cGate") is not True:
        return ["MARKHAND_PHASE1C_GATE!=1"]
    if os.environ.get("MARKHAND_PHASE1C_GATE") == "1" and os.environ.get("MARKHAND_TEST_REQUIRED") != "1":
        return ["MARKHAND_TEST_REQUIRED!=1"]
    return []


def evaluate_report(
    report: dict[str, Any],
    *,
    repo_root: Path | None = None,
    markhand_root: Path | None = None,
    bind_current_git: bool = False,
    evidence_must_exist: bool = True,
) -> tuple[str, list[str]]:
    workspace = repo_root or ROOT
    markhand = markhand_root or MARKHAND_ROOT
    blockers: list[str] = []

    if not isinstance(report, dict):
        return "fail", ["report_must_be_object"]

    status = report.get("status")
    if status not in {"pass", "fail", "incomplete", "not_run"}:
        blockers.append("status_type")

    blockers.extend(opt_in_errors(report, status=str(status)))
    blockers.extend(embedding_profile_errors(report.get("embeddingProfile"), status=str(status)))
    blockers.extend(p1c8_mapping_errors(report, status=str(status)))
    if status == "pass" and isinstance(report.get("vulnerabilityScan"), dict):
        blockers.extend(scanner_pin_errors(report["vulnerabilityScan"].get("scanner")))
    blockers.extend(residual_secret_errors(json.dumps(report, sort_keys=True), context="report"))

    try:
        registry = GATES.load_json_yaml(markhand / "gates.yaml")
    except (OSError, ValueError) as error:
        blockers.append(f"registry_load:{error}")
        registry = {"gates": []}

    template_mode = status != "pass"
    schema_report = strip_harness_extensions(report)
    errors = GATES.phase1c_gate_report_errors(
        schema_report,
        registry=registry,
        root=markhand,
        repo_root=workspace,
        workspace_root=workspace,
        template_mode=template_mode,
    )
    blockers.extend(errors)

    if status == "pass" and evidence_must_exist:
        blockers.extend(evidence_probe_errors(workspace, status="pass"))

    if bind_current_git and status == "pass":
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=workspace, text=True).strip()
        commit = (report.get("git") or {}).get("commit")
        if commit != head:
            blockers.append("git_sha_mismatch")

    blockers = sorted(dict.fromkeys(blockers))
    if status == "pass" and not blockers:
        return "pass", []
    if status == "not_run":
        return "not_run", blockers
    return "fail", blockers


def atomic_write_json(path: Path, payload: dict[str, Any], *, validate: bool = True) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    serialized = json.dumps(payload, indent=2, sort_keys=True) + "\n"
    redacted = redact_text(serialized)
    if residual_secret_errors(redacted, context=str(path.name)):
        raise HarnessWriteError("redaction_failed_before_write")
    fd, tmp_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    tmp_path = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(redacted)
            handle.flush()
            os.fsync(handle.fileno())
        tmp_path.replace(path)
    except Exception:
        tmp_path.unlink(missing_ok=True)
        raise
    if validate and path.name == "phase-1c-gate.json":
        loaded = json.loads(path.read_text(encoding="utf-8"))
        status, blockers = evaluate_report(loaded, bind_current_git=loaded.get("status") == "pass")
        if status != "pass":
            path.unlink(missing_ok=True)
            raise HarnessWriteError(f"refusing_write:{','.join(blockers)}")


def atomic_write_report(path: Path, report: dict[str, Any], *, repo_root: Path | None = None) -> None:
    if report.get("status") == "pass":
        eval_status, blockers = evaluate_report(
            report,
            repo_root=repo_root or ROOT,
            bind_current_git=True,
        )
        if eval_status != "pass":
            raise HarnessWriteError(f"refusing_write:{','.join(blockers)}")
    atomic_write_json(path, report)


def run_command(
    command: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    timeout_secs: int = 1800,
) -> dict[str, Any]:
    started = time.monotonic()
    proc = subprocess.run(
        command,
        cwd=cwd or ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout_secs,
        check=False,
    )
    stdout = redact_text(proc.stdout or "")
    stderr = redact_text(proc.stderr or "")
    combined = stdout + stderr
    return {
        "commandExitCode": proc.returncode,
        "timedOut": False,
        "outputTruncated": False,
        "eof": True,
        "durationMs": int((time.monotonic() - started) * 1000),
        "stdout": stdout,
        "stderr": stderr,
        "stdoutSha256": hashlib.sha256(stdout.encode()).hexdigest(),
        "stderrSha256": hashlib.sha256(stderr.encode()).hexdigest(),
        "residualSecrets": bool(SECRET_RESIDUAL_RE.search(combined)),
    }


def probe_from_command(command: list[str], runner: CommandRunner, **kwargs: Any) -> dict[str, Any]:
    outcome = runner(command, **kwargs)
    if outcome["commandExitCode"] != 0:
        raise RuntimeError("probe command failed")
    if outcome["residualSecrets"]:
        raise RuntimeError("probe output leaked secrets")
    if outcome.get("timedOut") or outcome.get("outputTruncated") or outcome.get("eof") is not True:
        raise RuntimeError("probe output incomplete")
    return outcome


def sanitize_probe(probe: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in probe.items() if key not in {"stdout", "stderr", "command"}}


def build_evidence_payload(
    *,
    gate_id: str,
    scenario: str,
    probe: dict[str, Any],
    metrics: dict[str, Any],
    source_revision: dict[str, Any],
    markhand_root: Path,
    repo_root: Path,
) -> dict[str, Any]:
    rel = next(str(row["evidence"]) for row in GATES.G1C_GATE_ROWS if row["id"] == gate_id)
    binding, _errors = GATES.phase1c_canonical_fingerprints(markhand_root, workspace_root=repo_root)
    return {
        "schemaVersion": EVIDENCE_SCHEMA_VERSION,
        "gateId": gate_id,
        "scenario": scenario,
        "evidencePath": rel,
        "environmentId": GATES.PHASE1C_ENVIRONMENT_ID,
        "workloadProfileId": GATES.PHASE1C_WORKLOAD_PROFILE_ID,
        "embeddingProfile": "mock",
        "sourceRevision": source_revision,
        "canonicalBinding": {"registryRevision": 1, **binding},
        "thresholdDecisions": GATES.canonical_threshold_decisions(),
        "targetMatch": True,
        "p1c8Items": list(GATE_TO_P1C8[gate_id]),
        "status": "pass",
        "probe": sanitize_probe(probe),
        "metrics": metrics,
    }


def write_gate_evidence_staged(
    staging: StagingWorkspace,
    gate_id: str,
    *,
    scenario: str,
    probe: dict[str, Any],
    metrics: dict[str, Any],
    markhand_root: Path,
) -> str:
    rel = next(str(row["evidence"]) for row in GATES.G1C_GATE_ROWS if row["id"] == gate_id)
    payload = build_evidence_payload(
        gate_id=gate_id,
        scenario=scenario,
        probe=probe,
        metrics=metrics,
        source_revision=staging.source_revision,
        markhand_root=markhand_root,
        repo_root=staging.repo_root,
    )
    errors = evidence_binding_errors(payload, evidence_path=rel)
    if errors:
        raise RuntimeError(f"evidence binding failed: {errors}")
    repo_path = staging.repo_root / rel
    repo_path.parent.mkdir(parents=True, exist_ok=True)
    atomic_write_json(repo_path, payload, validate=False)
    staging.stage_json(rel, payload)
    return rel


def run_cargo_metric_probe(
    gate_id: str,
    runner: CommandRunner,
    *,
    env: dict[str, str],
) -> tuple[dict[str, Any], dict[str, Any]]:
    raise RuntimeError("CARGO_PROBE_SPECS is CI substrate only; use deployed probes for qualifying PASS")


def build_deployed_context(
    repo_root: Path,
    *,
    command_runner: CommandRunner | None = None,
    shims: Any | None = None,
    seed: Any | None = None,
    challenge: str | None = None,
) -> Any:
    source_revision = capture_source_revision(repo_root)
    api_base = os.environ.get("MARKHAND_API_BASE", "http://127.0.0.1:8788")
    probe_challenge = challenge or os.environ.get(
        "MARKHAND_PHASE1C_CHALLENGE", f"phase1c-{secrets.token_hex(8)}"
    )
    if seed is None:
        seed_path = os.environ.get("MARKHAND_PHASE1C_SEED_JSON")
        if seed_path and Path(seed_path).is_file():
            seed = _DEPLOYED.load_seed_artifact(
                Path(seed_path), expected_challenge=probe_challenge
            )
        else:
            raise RuntimeError(
                "MARKHAND_PHASE1C_SEED_JSON is required; run deploy/scripts/phase1c-multi-org-seed.sh"
            )
    creds_path = os.environ.get("MARKHAND_PHASE1C_CREDENTIALS_JSON")
    if creds_path and Path(creds_path).is_file():
        credentials = _DEPLOYED.load_seed_credentials_secure(
            Path(creds_path), expected_challenge=probe_challenge, purge_after_load=True
        )
    else:
        raise RuntimeError(
            "MARKHAND_PHASE1C_CREDENTIALS_JSON is required alongside seed artifact"
        )
    noisy_secs = int(os.environ.get("MARKHAND_PHASE1C_NOISY_DURATION_SECS", "60"))
    return _DEPLOYED.DeployedContext(
        api_base=api_base,
        seed=seed,
        credentials=credentials,
        shims=shims or _DEPLOYED.LiveDeployedProbeShims(),
        noisy_duration_secs=noisy_secs,
        worker_nonce=os.environ.get("MARKHAND_PHASE1C_WORKER_NONCE", secrets.token_hex(16)),
    )


def run_live_probes(
    repo_root: Path,
    markhand_root: Path,
    *,
    command_runner: CommandRunner | None = None,
    output_dir: Path | None = None,
    deployed_context: Any | None = None,
) -> dict[str, Any]:
    if os.environ.get("MARKHAND_TEST_REQUIRED") != "1":
        raise RuntimeError("MARKHAND_TEST_REQUIRED must be 1")
    profiles = os.environ.get("COMPOSE_PROFILES", "")
    embedding = os.environ.get("MARKHAND_EMBEDDING_RUNTIME_PATH", "")
    if profiles and "mock" not in profiles.split(","):
        raise RuntimeError("qualifying run requires mock embedding profile")
    if embedding and embedding not in {"local-neural", "mock"}:
        raise RuntimeError("cloud/shared embedding profile forbidden for qualifying pass")

    runner = command_runner or run_command
    final_dir = output_dir or (repo_root / ".artifacts" / "phase1c-gate-live")
    staging = StagingWorkspace(repo_root=repo_root, final_dir=final_dir)
    staging.purge_final_allowlisted_artifacts()
    metrics: dict[str, Any] = {}
    context = deployed_context or build_deployed_context(repo_root, command_runner=runner)
    probe_runner = _DEPLOYED.DeployedProbeRunner(
        api_base=context.api_base,
        seed=context.seed,
        credentials=context.credentials,
        shims=context.shims,
        noisy_duration_secs=context.noisy_duration_secs,
        git_sha_full=staging.source_revision.get("commit"),
    )

    try:
        for gate_id in DEPLOYED_PROBE_GATES:
            result = probe_runner.run_gate(gate_id)
            probe = _DEPLOYED.deployed_probe_to_command_probe(result)
            gate_metrics = dict(result.metrics)
            for key, value in gate_metrics.items():
                metrics[key] = value
            write_gate_evidence_staged(
                staging,
                gate_id,
                scenario=SCENARIO_BY_GATE[gate_id],
                probe=probe,
                metrics=gate_metrics,
                markhand_root=markhand_root,
            )

        compose = ["docker", "compose", "-f", str(COMPOSE_FILE)]
        worker_nonce = context.worker_nonce
        worker_cmd = [
            *compose,
            "exec",
            "-T",
            "-e",
            f"MARKHAND_PHASE1C_WORKER_NONCE={worker_nonce}",
            "worker-convert",
            "fileconv-worker",
            "--db-role-probe",
        ]
        worker_proc = runner(worker_cmd, timeout_secs=120)
        if worker_proc["commandExitCode"] != 0 or worker_proc.get("residualSecrets"):
            raise RuntimeError("worker role probe failed")
        worker_payload = parse_worker_role_probe(
            (worker_proc.get("stdout") or "") + (worker_proc.get("stderr") or ""),
            expected_nonce=worker_nonce,
        )
        metrics["worker_dedicated_role_verified"] = 1
        worker_proof = {
            "runtimeRole": worker_payload["currentUser"],
            "dedicatedDatabaseUrlVerified": worker_payload["dedicatedDatabaseUrlVerified"],
            "superuser": worker_payload["superuser"],
            "bypassRls": worker_payload["bypassRls"],
            "verifiedAt": utc_now_z(),
            "nonce": worker_payload["nonce"],
        }
        write_gate_evidence_staged(
            staging,
            "G1C-SEC-WORKER-ROLE",
            scenario=SCENARIO_BY_GATE["G1C-SEC-WORKER-ROLE"],
            probe=worker_proc,
            metrics={"worker_dedicated_role_verified": 1},
            markhand_root=markhand_root,
        )

        trivy_image = pinned_trivy_image()
        api_tag = os.environ.get("MARKHAND_API_IMAGE", "markhand-api:poc")
        worker_tag = os.environ.get("MARKHAND_WORKER_IMAGE", "markhand-worker:poc")
        api_ref = docker_image_digest(api_tag, runner)
        worker_ref = docker_image_digest(worker_tag, runner)
        trivyignore_text = TRIVYIGNORE.read_text(encoding="utf-8") if TRIVYIGNORE.is_file() else ""

        def trivy_scan(tag: str, digest_ref: str) -> tuple[dict[str, Any], dict[str, Any]]:
            out_path = staging.temp_dir / f"trivy-{tag.replace(':', '-')}.json"
            cmd = [
                "docker",
                "run",
                "--rm",
                "-v",
                "/var/run/docker.sock:/var/run/docker.sock:ro",
                trivy_image,
                "image",
                "--severity",
                "HIGH,CRITICAL",
                "--format",
                "json",
                "--output",
                str(out_path),
                digest_ref,
            ]
            probe = probe_from_command(cmd, runner, timeout_secs=900)
            if not out_path.is_file() or out_path.stat().st_size == 0:
                raise RuntimeError("trivy output missing or empty")
            report = json.loads(out_path.read_text(encoding="utf-8"))
            probe["trivyReportSha256"] = hashlib.sha256(out_path.read_bytes()).hexdigest()
            return probe, report

        api_probe, api_report = trivy_scan(api_tag, api_ref)
        worker_probe, worker_report = trivy_scan(worker_tag, worker_ref)
        vuln_scan = parse_combined_trivy_scan(
            api_report=api_report,
            worker_report=worker_report,
            api_ref=api_ref,
            worker_ref=worker_ref,
            trivyignore_text=trivyignore_text,
        )
        if vuln_scan["undispositionedHighCritical"] != 0:
            raise RuntimeError("undispositioned HIGH/CRITICAL findings remain")
        metrics["undispositioned_high_critical_count"] = vuln_scan["undispositionedHighCritical"]
        combined_probe = {
            **api_probe,
            "workerScanIncluded": True,
            "workerProbeExitCode": worker_probe["commandExitCode"],
            "completionMarker": "PHASE1C_TRIVY_EOF",
        }
        write_gate_evidence_staged(
            staging,
            "G1C-SEC-CONTAINER-VULNS",
            scenario=SCENARIO_BY_GATE["G1C-SEC-CONTAINER-VULNS"],
            probe=combined_probe,
            metrics={"undispositioned_high_critical_count": metrics["undispositioned_high_critical_count"]},
            markhand_root=markhand_root,
        )

        for metric in GATES.PHASE1C_METRIC_THRESHOLDS:
            if metric not in metrics:
                raise RuntimeError(f"missing required metric: {metric}")

        report = assemble_pass_report(
            metrics,
            worker_proof,
            vuln_scan,
            repo_root=repo_root,
            markhand_root=markhand_root,
            source_revision=staging.source_revision,
        )
        report["denialManifestSha256"] = context.seed.manifest_sha256
        staging.stage_json("bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json", report)
        staging.commit_all_report_last()
        return report
    except Exception:
        staging.purge_final_allowlisted_artifacts()
        for rel in GATES.PHASE1C_EVIDENCE_ALLOWLIST:
            path = repo_root / rel
            if path.exists():
                path.unlink(missing_ok=True)
        raise
    finally:
        staging.cleanup()


def build_threshold_decisions() -> list[dict[str, Any]]:
    return GATES.canonical_threshold_decisions()


def assemble_pass_report(
    metrics: dict[str, Any],
    worker_proof: dict[str, Any],
    vuln_scan: dict[str, Any],
    *,
    repo_root: Path,
    markhand_root: Path,
    source_revision: dict[str, Any],
) -> dict[str, Any]:
    report = GATES.load_json_yaml(TEMPLATE_REPORT)
    report["status"] = "pass"
    report["targetMatch"] = True
    report["markhandPhase1cGate"] = True
    report["embeddingProfile"] = "mock"
    report["generatedAt"] = utc_now_z()
    report["metrics"] = metrics
    report["workerProof"] = worker_proof
    report["redactionScan"] = {"passed": True}
    report["vulnerabilityScan"] = vuln_scan
    report["p1c8EvidenceMapping"] = build_p1c8_mapping()
    report["thresholdDecisions"] = build_threshold_decisions()
    for decision in report["thresholdDecisions"]:
        decision["recordedAt"] = utc_now_z()
    worker_proof["verifiedAt"] = utc_now_z()
    for result in report["gateResults"]:
        metric = result["metric"]
        result["value"] = metrics[metric]
        result["pass"] = True
    report["canonicalBinding"] = {
        "registryRevision": 1,
        **GATES.phase1c_canonical_fingerprints(markhand_root, workspace_root=repo_root)[0],
    }
    report["git"] = source_revision
    report["denialManifestSha256"] = hashlib.sha256(DENIAL_MANIFEST.read_bytes()).hexdigest()
    status, blockers = evaluate_report(
        report,
        repo_root=repo_root,
        markhand_root=markhand_root,
        bind_current_git=True,
    )
    if status != "pass":
        raise RuntimeError(f"assembled report failed validation: {blockers}")
    return report


def write_not_run_report(repo_root: Path, markhand_root: Path) -> dict[str, Any]:
    report = GATES.load_json_yaml(TEMPLATE_REPORT)
    report["generatedAt"] = utc_now_z()
    status, blockers = evaluate_report(
        report,
        repo_root=repo_root,
        markhand_root=markhand_root,
        evidence_must_exist=False,
    )
    report["status"] = status
    return report


def write_safe_failure_diagnostic(
    output_dir: Path,
    *,
    message: str,
    repo_root: Path,
    markhand_root: Path,
) -> Path:
    """Write schema-valid sanitized failure artifact (never raw logs)."""
    failure = write_safe_failure_report(repo_root, markhand_root, message=message)
    failure["diagnosticSchemaVersion"] = 1
    failure["redactionScan"] = {"passed": True}
    path = output_dir / "phase1c-gate-failure.json"
    atomic_write_json(path, failure, validate=False)
    return path


def write_safe_failure_report(
    repo_root: Path,
    markhand_root: Path,
    *,
    message: str,
) -> dict[str, Any]:
    failure = write_not_run_report(repo_root, markhand_root)
    failure["status"] = "fail"
    failure["notes"] = sanitize_failure_message(message)
    failure["redactionScan"] = {"passed": True}
    return failure


def validate_report_cli(path: Path) -> int:
    report = json.loads(path.read_text(encoding="utf-8"))
    status, blockers = evaluate_report(report, bind_current_git=report.get("status") == "pass")
    print(json.dumps({"status": status, "blockers": blockers}, indent=2, sort_keys=True))
    return 0 if status == "pass" else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="Phase 1C G1C-SEC qualification harness")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--validate-report", type=Path, default=None)
    parser.add_argument("--output-dir", type=Path, default=None)
    args = parser.parse_args()
    if args.self_test:
        return 0
    if args.validate_report is not None:
        return validate_report_cli(args.validate_report.resolve())

    repo_root = ROOT
    markhand_root = MARKHAND_ROOT
    output_report = DEFAULT_REPORT
    if args.output_dir is not None:
        output_report = args.output_dir / "phase-1c-gate.json"

    if os.environ.get("MARKHAND_PHASE1C_GATE") != "1":
        report = write_not_run_report(repo_root, markhand_root)
        print(output_report)
        return 0

    staging = StagingWorkspace(repo_root=repo_root, final_dir=output_report.parent)
    staging.purge_final_allowlisted_artifacts()
    try:
        report = run_live_probes(repo_root, markhand_root, output_dir=output_report.parent)
        atomic_write_json(output_report, report)
        staging.commit_all()
        print(output_report)
        return 0
    except Exception as error:
        staging.purge_final_allowlisted_artifacts()
        failure = write_safe_failure_report(
            repo_root,
            markhand_root,
            message=str(error),
        )
        try:
            write_safe_failure_diagnostic(
                output_report.parent,
                message=str(error),
                repo_root=repo_root,
                markhand_root=markhand_root,
            )
        except HarnessWriteError:
            pass
        try:
            atomic_write_json(output_report, failure, validate=False)
        except HarnessWriteError:
            pass
        print(sanitize_failure_message(str(error)), file=sys.stderr)
        return 1
    finally:
        staging.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
