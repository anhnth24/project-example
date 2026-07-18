#!/usr/bin/env python3
"""P0-09 upload security policy and sandbox smoke harness.

This harness validates policy evidence and performs in-process denial
simulations. It deliberately does not claim that a container runtime or malware
scanner executed.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[3]
CORPUS = ROOT / "bench/markhand_web"
MANIFEST_PATH = CORPUS / "adversarial/manifest.json"
DISPOSITION_PATH = CORPUS / "security/adversarial-disposition.json"
SANDBOX_PROFILE_PATH = CORPUS / "security/sandbox-profile.json"
POLICY_PATH = ROOT / "docs/markhand-web-upload-policy.md"
THREAT_MODEL_PATH = ROOT / "docs/markhand-web-upload-threat-model.md"
SUMMARY_PATH = CORPUS / "security/summary.json"
REPORT_PATH = CORPUS / "reports/upload-security.md"
LICENSE_CHECKER = ROOT / "scripts/check-runtime-license-inventory.py"

REQUIRED_POLICY_SECTIONS = (
    "allowlist",
    "upload limits",
    "quarantine lifecycle",
    "sandbox profile",
    "tenant, token, and quota controls",
    "runtime license policy",
)
REQUIRED_POLICY_KEYWORDS = (
    "allowlist",
    "limits",
    "quarantine",
    "sandbox",
    "non-root",
    "read-only root",
    "no egress",
    "cpu",
    "ram",
    "file",
    "process",
    "wall",
)
REQUIRED_THREAT_KEYWORDS = (
    "spoof",
    "bomb",
    "parser",
    "ssrf",
    "exhaustion",
    "traversal",
    "injection",
    "token",
    "quota",
    "tenant",
    "compromised worker",
)


class HarnessError(RuntimeError):
    """Actionable validation failure."""


def load_json(path: Path) -> dict:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise HarnessError(f"{path}: cannot load JSON: {error}") from error
    if not isinstance(payload, dict):
        raise HarnessError(f"{path}: expected JSON object")
    return payload


def file_sha256(path: Path) -> str:
    import hashlib

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_status() -> dict:
    try:
        branch = subprocess.check_output(
            ["git", "branch", "--show-current"], cwd=ROOT, text=True
        ).strip()
        raw = subprocess.check_output(
            ["git", "status", "--porcelain"], cwd=ROOT, text=True
        )
    except (OSError, subprocess.CalledProcessError):
        return {"branch": "unknown", "clean": False, "dirtyPaths": ["git-status-unavailable"]}

    dirty_paths: list[str] = []
    for line in raw.splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        if path.startswith('"') and path.endswith('"'):
            path = path[1:-1]
        dirty_paths.append(path)
    return {"branch": branch, "clean": not dirty_paths, "dirtyPaths": dirty_paths}


def validate_adversarial_disposition(manifest: dict, disposition: dict) -> dict:
    attacks = manifest.get("attacks")
    mapped = disposition.get("attacks")
    errors: list[str] = []
    if not isinstance(attacks, list) or not attacks:
        raise HarnessError("adversarial manifest missing attacks array")
    if not isinstance(mapped, dict):
        raise HarnessError("adversarial disposition missing attacks object")

    manifest_by_id = {}
    for attack in attacks:
        attack_id = attack.get("id") if isinstance(attack, dict) else None
        if not isinstance(attack_id, str) or not attack_id:
            errors.append("manifest attack missing id")
            continue
        manifest_by_id[attack_id] = attack

    manifest_ids = set(manifest_by_id)
    mapped_ids = set(mapped)
    missing = sorted(manifest_ids - mapped_ids)
    unexpected = sorted(mapped_ids - manifest_ids)
    if missing:
        errors.append(f"missing disposition ids: {missing}")
    if unexpected:
        errors.append(f"unexpected disposition ids: {unexpected}")

    rows = []
    passed = 0
    for attack_id in sorted(manifest_ids):
        attack = manifest_by_id[attack_id]
        expected = attack.get("expectedDisposition")
        item = mapped.get(attack_id, {})
        actual = item.get("disposition") if isinstance(item, dict) else None
        controls = item.get("controls") if isinstance(item, dict) else None
        ok = actual == expected and actual in {"reject", "quarantine", "allow"}
        if not isinstance(controls, list) or not controls or not all(
            isinstance(control, str) and control.strip() for control in controls
        ):
            ok = False
            errors.append(f"{attack_id}: controls must be non-empty strings")
        if actual != expected:
            errors.append(f"{attack_id}: disposition {actual!r} != expected {expected!r}")
        if ok:
            passed += 1
        rows.append(
            {
                "id": attack_id,
                "threatClass": attack.get("threatClass"),
                "expectedDisposition": expected,
                "actualDisposition": actual,
                "pass": ok,
            }
        )

    total = len(manifest_ids)
    return {
        "pass": not errors and total == len(mapped_ids),
        "passed": passed,
        "total": total,
        "ratio": round(passed / total, 6) if total else 0.0,
        "errors": errors,
        "rows": rows,
    }


def validate_policy(policy_path: Path, threat_model_path: Path) -> dict:
    errors: list[str] = []
    try:
        policy = policy_path.read_text(encoding="utf-8")
    except OSError as error:
        raise HarnessError(f"{policy_path}: cannot read policy: {error}") from error
    try:
        threat_model = threat_model_path.read_text(encoding="utf-8")
    except OSError as error:
        raise HarnessError(f"{threat_model_path}: cannot read threat model: {error}") from error

    lower_policy = policy.lower()
    lower_threat = threat_model.lower()
    for section in REQUIRED_POLICY_SECTIONS:
        if section not in lower_policy:
            errors.append(f"policy missing section/phrase: {section}")
    for keyword in REQUIRED_POLICY_KEYWORDS:
        if keyword not in lower_policy:
            errors.append(f"policy missing keyword: {keyword}")
    for keyword in REQUIRED_THREAT_KEYWORDS:
        if keyword not in lower_threat:
            errors.append(f"threat model missing keyword: {keyword}")
    for owner_word in ("prevention", "detection", "owner"):
        if owner_word not in lower_threat:
            errors.append(f"threat model missing {owner_word}")
    return {
        "pass": not errors,
        "errors": errors,
        "policySha256": file_sha256(policy_path),
        "threatModelSha256": file_sha256(threat_model_path),
    }


def positive_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def validate_sandbox_profile(profile: dict) -> dict:
    errors: list[str] = []
    user = profile.get("runtimeUser", {})
    filesystem = profile.get("filesystem", {})
    network = profile.get("network", {})
    process = profile.get("process", {})
    limits = profile.get("resourceLimits", {})

    if user.get("nonRoot") is not True:
        errors.append("sandbox runtimeUser.nonRoot must be true")
    if not positive_int(user.get("uidMin")) or user["uidMin"] < 10000:
        errors.append("sandbox uidMin must be >= 10000")
    if not positive_int(user.get("gidMin")) or user["gidMin"] < 10000:
        errors.append("sandbox gidMin must be >= 10000")
    if filesystem.get("rootReadOnly") is not True:
        errors.append("sandbox root filesystem must be read-only")
    if filesystem.get("hostPathMountsAllowed") is not False:
        errors.append("sandbox host path mounts must be denied")
    if filesystem.get("inputMountReadOnly") is not True:
        errors.append("sandbox input mount must be read-only")
    write_mounts = filesystem.get("allowedWriteMounts")
    if not isinstance(write_mounts, list) or not write_mounts:
        errors.append("sandbox must define at least one bounded write mount")
    else:
        for mount in write_mounts:
            if mount.get("type") != "tmpfs" or not positive_int(mount.get("maxBytes")):
                errors.append("sandbox write mounts must be bounded tmpfs mounts")
    for key in ("egressAllowed", "ingressAllowed", "dnsAllowed", "loopbackAllowed"):
        if network.get(key) is not False:
            errors.append(f"sandbox network.{key} must be false")
    if process.get("dropCapabilities") != "all":
        errors.append("sandbox must drop all capabilities")
    if process.get("noNewPrivileges") is not True:
        errors.append("sandbox noNewPrivileges must be true")
    if process.get("killProcessGroupOnExit") is not True:
        errors.append("sandbox must kill process group on exit")
    if not positive_int(process.get("maxProcesses")) or process["maxProcesses"] > 128:
        errors.append("sandbox maxProcesses must be within 1..128")
    if not positive_int(process.get("maxOpenFiles")) or process["maxOpenFiles"] > 1024:
        errors.append("sandbox maxOpenFiles must be within 1..1024")

    required_limits = {
        "cpuSeconds": 600,
        "memoryMiB": 4096,
        "fileBytes": 1024 * 1024 * 1024,
        "outputBytes": 512 * 1024 * 1024,
        "wallClockSeconds": 900,
        "archiveEntries": 10000,
        "archiveUncompressedBytes": 2 * 1024 * 1024 * 1024,
        "archiveCompressionRatioMax": 200,
        "pdfPagesMax": 1000,
        "audioDurationSecondsMax": 7200,
        "imagePixelsMax": 200000000,
    }
    for key, maximum in required_limits.items():
        value = limits.get(key)
        if not positive_int(value) or value > maximum:
            errors.append(f"sandbox resourceLimits.{key} missing or exceeds {maximum}")
    return {"pass": not errors, "errors": errors, "profileSha256": file_sha256(SANDBOX_PROFILE_PATH)}


def deny_egress(profile: dict, target: str) -> bool:
    network = profile.get("network", {})
    return (
        target.startswith(("http://", "https://", "tcp://", "udp://"))
        and network.get("egressAllowed") is False
        and network.get("dnsAllowed") is False
    )


def deny_traversal(profile: dict, member_path: str) -> bool:
    filesystem = profile.get("filesystem", {})
    pure = PurePosixPath(member_path)
    has_drive = ":" in member_path.split("/", 1)[0]
    unsafe = pure.is_absolute() or ".." in pure.parts or has_drive
    return unsafe and filesystem.get("hostPathMountsAllowed") is False


def deny_fork_bomb(profile: dict, requested_processes: int) -> bool:
    max_processes = profile.get("process", {}).get("maxProcesses")
    return positive_int(max_processes) and requested_processes > max_processes


def deny_timeout(profile: dict, requested_wall_seconds: int) -> bool:
    wall = profile.get("resourceLimits", {}).get("wallClockSeconds")
    return positive_int(wall) and requested_wall_seconds > wall


def run_denial_simulations(profile: dict) -> dict:
    checks = {
        "egressDenied": deny_egress(profile, "https://metadata.invalid/latest/token"),
        "traversalDenied": deny_traversal(profile, "../../tenant-b/private.docx"),
        "forkBombDenied": deny_fork_bomb(profile, 100000),
        "timeoutDenied": deny_timeout(profile, 86400),
    }
    return {"pass": all(checks.values()), "checks": checks}


def run_license_checker() -> dict:
    try:
        completed = subprocess.run(
            [sys.executable, str(LICENSE_CHECKER)],
            cwd=ROOT,
            text=True,
            capture_output=True,
            timeout=20,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return {"pass": False, "returnCode": None, "stdout": "", "stderr": str(error)}
    return {
        "pass": completed.returncode == 0,
        "returnCode": completed.returncode,
        "stdout": completed.stdout.strip(),
        "stderr": completed.stderr.strip(),
    }


def build_payload() -> dict:
    git = git_status()
    manifest = load_json(MANIFEST_PATH)
    disposition = load_json(DISPOSITION_PATH)
    sandbox_profile = load_json(SANDBOX_PROFILE_PATH)
    adversarial = validate_adversarial_disposition(manifest, disposition)
    policy = validate_policy(POLICY_PATH, THREAT_MODEL_PATH)
    sandbox = validate_sandbox_profile(sandbox_profile)
    simulations = run_denial_simulations(sandbox_profile)
    license_checker = run_license_checker()

    closure = {
        "adversarialDispositionComplete": adversarial["pass"],
        "policyLinterPassed": policy["pass"],
        "sandboxProfilePassed": sandbox["pass"],
        "denialSimulationsPassed": simulations["pass"],
        "licenseCheckerWouldPass": license_checker["pass"],
        "gitClean": git["clean"],
    }
    return {
        "version": 1,
        "reportId": "p0-09-upload-security",
        "gateId": "G0-SEC-UPLOAD-DENIAL",
        "command": "python3 bench/markhand_web/scripts/run_upload_security.py",
        "measurementScope": "local-cpu policy/sandbox smoke; not Profile B malware scanner",
        "environment": {
            "environmentId": "local-cpu-quality",
            "targetMatch": True,
            "profileBMalwareScannerEvidence": False,
        },
        "git": git,
        "inputs": {
            "manifest": str(MANIFEST_PATH.relative_to(ROOT)),
            "disposition": str(DISPOSITION_PATH.relative_to(ROOT)),
            "policy": str(POLICY_PATH.relative_to(ROOT)),
            "threatModel": str(THREAT_MODEL_PATH.relative_to(ROOT)),
            "sandboxProfile": str(SANDBOX_PROFILE_PATH.relative_to(ROOT)),
        },
        "adversarial": adversarial,
        "policy": policy,
        "sandbox": sandbox,
        "denialSimulations": simulations,
        "licenseChecker": license_checker,
        "closure": closure,
        "p0_09_closed": all(closure.values()),
        "doesNotClaim": [
            "malware scanner coverage",
            "container runtime enforcement",
            "Profile B production hardening"
        ],
        "notes": [
            "G0-SEC uses local-cpu-quality for policy/sandbox smoke evidence.",
            "No adversarial fixture is parsed or executed by this harness.",
            "Production upload workers must implement the sandbox profile before accepting user uploads."
        ],
    }


def render_report(payload: dict) -> str:
    lines = [
        "# P0-09 upload security report",
        "",
        f"- Scope: `{payload['measurementScope']}`",
        f"- Environment: `{payload['environment']['environmentId']}`",
        f"- Git clean at harness start: `{str(payload['git']['clean']).lower()}`",
        f"- `p0_09_closed`: `{str(payload['p0_09_closed']).lower()}`",
        "",
        "## Closure",
        "",
        "| field | value |",
        "|---|---|",
    ]
    for key, value in payload["closure"].items():
        lines.append(f"| `{key}` | `{str(value).lower()}` |")

    lines.extend(
        [
            "",
            "## Adversarial fixture dispositions",
            "",
            f"- Passed: `{payload['adversarial']['passed']}/{payload['adversarial']['total']}`",
            f"- Ratio: `{payload['adversarial']['ratio']}`",
            "",
            "| attack | threat class | expected | actual | pass |",
            "|---|---|---|---|---|",
        ]
    )
    for row in payload["adversarial"]["rows"]:
        lines.append(
            "| "
            f"`{row['id']}` | `{row['threatClass']}` | `{row['expectedDisposition']}` | "
            f"`{row['actualDisposition']}` | `{str(row['pass']).lower()}` |"
        )

    lines.extend(
        [
            "",
            "## Denial simulations",
            "",
            "These are in-process policy checks, not container runtime execution.",
            "",
            "| check | denied |",
            "|---|---|",
        ]
    )
    for key, value in payload["denialSimulations"]["checks"].items():
        lines.append(f"| `{key}` | `{str(value).lower()}` |")

    lines.extend(
        [
            "",
            "## License checker",
            "",
            f"- Pass: `{str(payload['licenseChecker']['pass']).lower()}`",
            f"- stdout: `{payload['licenseChecker']['stdout']}`",
            "",
            "## Scope notes",
            "",
        ]
    )
    for note in payload["notes"]:
        lines.append(f"- {note}")
    for claim in payload["doesNotClaim"]:
        lines.append(f"- Does not claim: {claim}.")
    if payload["git"]["dirtyPaths"]:
        lines.extend(["", "Dirty paths at harness start:"])
        for path in payload["git"]["dirtyPaths"]:
            lines.append(f"- `{path}`")
    for section in ("adversarial", "policy", "sandbox"):
        errors = payload[section].get("errors", [])
        if errors:
            lines.extend(["", f"## {section} errors", ""])
            lines.extend(f"- {error}" for error in errors)
    if payload["licenseChecker"]["stderr"]:
        lines.extend(["", "## License checker stderr", "", payload["licenseChecker"]["stderr"]])
    lines.append("")
    return "\n".join(lines)


def write_outputs(payload: dict, summary: Path, report: Path) -> None:
    summary.parent.mkdir(parents=True, exist_ok=True)
    report.parent.mkdir(parents=True, exist_ok=True)
    summary.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    report.write_text(render_report(payload), encoding="utf-8")


def self_test() -> None:
    sample_profile = {
        "runtimeUser": {"nonRoot": True, "uidMin": 10000, "gidMin": 10000},
        "filesystem": {
            "rootReadOnly": True,
            "hostPathMountsAllowed": False,
            "inputMountReadOnly": True,
            "allowedWriteMounts": [{"path": "/work", "type": "tmpfs", "maxBytes": 1024}],
        },
        "network": {
            "egressAllowed": False,
            "ingressAllowed": False,
            "dnsAllowed": False,
            "loopbackAllowed": False,
        },
        "process": {
            "dropCapabilities": "all",
            "noNewPrivileges": True,
            "killProcessGroupOnExit": True,
            "maxProcesses": 64,
            "maxOpenFiles": 256,
        },
        "resourceLimits": {
            "cpuSeconds": 300,
            "memoryMiB": 1536,
            "fileBytes": 209715200,
            "outputBytes": 52428800,
            "wallClockSeconds": 420,
            "archiveEntries": 4096,
            "archiveUncompressedBytes": 1073741824,
            "archiveCompressionRatioMax": 100,
            "pdfPagesMax": 500,
            "audioDurationSecondsMax": 3600,
            "imagePixelsMax": 80000000,
        },
    }
    assert deny_egress(sample_profile, "https://example.invalid") is True
    assert deny_traversal(sample_profile, "../x") is True
    assert deny_traversal(sample_profile, "/etc/passwd") is True
    assert deny_traversal(sample_profile, "C:/Users/file") is True
    assert deny_traversal(sample_profile, "safe/document.xml") is False
    assert deny_fork_bomb(sample_profile, 65) is True
    assert deny_fork_bomb(sample_profile, 64) is False
    assert deny_timeout(sample_profile, 421) is True
    assert run_denial_simulations(sample_profile)["pass"] is True

    manifest = {
        "attacks": [
            {"id": "a", "threatClass": "spoof", "expectedDisposition": "reject"},
            {"id": "b", "threatClass": "injection", "expectedDisposition": "quarantine"},
        ]
    }
    disposition = {
        "attacks": {
            "a": {"disposition": "reject", "controls": ["allowlist"]},
            "b": {"disposition": "quarantine", "controls": ["quarantine"]},
        }
    }
    assert validate_adversarial_disposition(manifest, disposition)["pass"] is True
    bad = {"attacks": {"a": {"disposition": "allow", "controls": ["bad"]}}}
    assert validate_adversarial_disposition(manifest, bad)["pass"] is False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--summary", type=Path, default=SUMMARY_PATH)
    parser.add_argument("--report", type=Path, default=REPORT_PATH)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        print("self-test ok")
        return 0

    try:
        payload = build_payload()
        write_outputs(payload, args.summary.resolve(), args.report.resolve())
    except HarnessError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    print(f"wrote {args.summary.resolve().relative_to(ROOT)}")
    print(f"wrote {args.report.resolve().relative_to(ROOT)}")
    print(f"blocked_attack_fixtures={payload['adversarial']['ratio']}")
    print(f"p0_09_closed={str(payload['p0_09_closed']).lower()}")
    # Fail-closed: any closure check failure exits non-zero.
    return 0 if payload["p0_09_closed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
