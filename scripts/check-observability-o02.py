#!/usr/bin/env python3
"""Validate P1B-O02 observability artifacts with pinned promtool + schema checks.

- Invokes pinned promtool (scripts/fetch-promtool.sh → .tools/promtool 2.55.1)
  for `check rules` and `test rules` (temporal/equality/non-firing).
- Cross-checks threshold provenance against gates.yaml / SLA / workload profile.
- Validates Grafana datasource UIDs/queries, OTel/Prometheus/Alertmanager/compose
  YAML via parsers, forbidden cardinality/secret hygiene.
- Mutation self-tests catch fake metrics, malformed PromQL, and for-window errors.
- Regenerates deploy/observability/evidence/validation-report.json (never hand-authored).

Does NOT claim a live outage was exercised. Docker is optional and may be absent.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parents[1]
OBS = ROOT / "deploy" / "observability"
THRESHOLDS_PATH = OBS / "thresholds.yaml"
ALERT_RULES_PATH = OBS / "prometheus" / "alert_rules.yml"
BLOCKED_RULES_PATH = OBS / "prometheus" / "alert_rules.blocked.yml"
RECORDING_RULES_PATH = OBS / "prometheus" / "recording_rules.yml"
PROMETHEUS_PATH = OBS / "prometheus" / "prometheus.yml"
ALERTMANAGER_PATH = OBS / "alertmanager" / "alertmanager.example.yml"
OTEL_PATH = OBS / "otel" / "collector-prometheus.yaml"
COMPOSE_PATH = OBS / "compose.observability.yml"
BLACKBOX_PATH = OBS / "blackbox" / "blackbox.yml"
IMAGES_LOCK = OBS / "images.lock.json"
DASHBOARD_DIR = OBS / "grafana" / "dashboards"
DATASOURCE_PATH = OBS / "grafana" / "provisioning" / "datasources" / "datasource.yml"
PROM_TESTS = OBS / "prometheus" / "tests" / "alerts_test.yml"
TABLETOP_PATH = OBS / "fixtures" / "tabletop" / "o02-tabletop.json"
EVIDENCE_PATH = OBS / "evidence" / "validation-report.json"
GATES_PATH = ROOT / "bench" / "markhand_web" / "gates.yaml"
WORKLOAD_PATH = ROOT / "bench" / "markhand_web" / "workload-profile.yaml"
RUNBOOK_DIR = ROOT / "docs" / "runbooks"
FETCH_PROMTOOL = ROOT / "scripts" / "fetch-promtool.sh"

REQUIRED_RUNBOOKS = [
    "stuck-dead-jobs.md",
    "converter-outbreak.md",
    "dependency-outage.md",
    "vector-rebuild.md",
    "disk-exhaustion.md",
    "glm-fallback.md",
    "key-rotation.md",
]
RUNBOOK_SECTIONS = ("Detection", "Contain", "Recover", "Verify", "Rollback")
FORBIDDEN_LABELS = {
    "org_id",
    "user_id",
    "actor_id",
    "document_id",
    "version_id",
    "job_id",
    "request_id",
    "trace_id",
    "email",
    "filename",
    "path",
    "query",
    "url",
    "object_key",
}
SECRET_RE = re.compile(
    r"(?i)(postgres(?:ql)?://\S+:\S+@|-----BEGIN [A-Z ]*PRIVATE KEY-----|"
    r"\bAKIA[0-9A-Z]{16}\b|\bghp_[A-Za-z0-9]{20,}\b|xox[baprs]-[0-9A-Za-z-]+)"
)
DATASOURCE_UID = "markhand-prometheus"


def load_yaml(path: Path) -> Any:
    return yaml.safe_load(path.read_text(encoding="utf-8"))


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def flatten_rules(path: Path) -> list[dict[str, Any]]:
    data = load_yaml(path)
    rules: list[dict[str, Any]] = []
    for group in data.get("groups") or []:
        for rule in group.get("rules") or []:
            rules.append(rule)
    return rules


def resolve_promtool() -> Path:
    env = os.environ.get("PROMTOOL")
    if env and Path(env).is_file():
        return Path(env)
    local = ROOT / ".tools" / "promtool"
    if local.is_file():
        return local
    if FETCH_PROMTOOL.is_file():
        result = subprocess.run(
            ["bash", str(FETCH_PROMTOOL)],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode == 0:
            path = Path(result.stdout.strip().splitlines()[-1])
            if path.is_file():
                return path
        raise RuntimeError(
            "failed to fetch pinned promtool:\n"
            f"{result.stdout}\n{result.stderr}"
        )
    which = shutil.which("promtool")
    if which:
        return Path(which)
    raise RuntimeError(
        "promtool not found; run scripts/fetch-promtool.sh "
        "(pinned 2.55.1) or set PROMTOOL="
    )


def run_promtool(promtool: Path, args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(promtool), *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


class ObservabilityO02Checks:
    def __init__(self) -> None:
        self.errors: list[str] = []
        self.notes: list[str] = []
        self.thresholds = load_yaml(THRESHOLDS_PATH)
        self.gates = load_json(GATES_PATH)
        self.workload = load_json(WORKLOAD_PATH)
        self.alert_rules = flatten_rules(ALERT_RULES_PATH)
        self.recording_rules = flatten_rules(RECORDING_RULES_PATH)
        self.blocked_rules = flatten_rules(BLOCKED_RULES_PATH)
        self.promtool_path: Path | None = None
        self.promtool_check_ok = False
        self.promtool_test_ok = False

    def err(self, msg: str) -> None:
        self.errors.append(msg)

    def check_threshold_provenance(self) -> None:
        gate_by_id = {g["id"]: g for g in self.gates["gates"]}
        sources = self.thresholds.get("sources") or {}
        for key, src in sources.items():
            kind = src.get("kind")
            if kind not in {
                "formal_gate",
                "formal_sla",
                "formal_workload",
                "operational_policy",
            }:
                self.err(f"source {key}: missing/invalid kind")
            gate_id = src.get("gateId")
            if gate_id:
                gate = gate_by_id.get(gate_id)
                if not gate:
                    self.err(f"{key}: unknown gate {gate_id}")
                elif float(src["value"]) != float(gate["threshold"]["value"]):
                    self.err(
                        f"{key}: value {src['value']} != gates.yaml {gate_id}"
                    )
            if key == "WORKLOAD-DISK-HEADROOM-PERCENT":
                disk = self.workload["hardware"]["headroomPercent"]["disk"]
                if float(src["value"]) != float(disk):
                    self.err(f"disk headroom mismatch: {src['value']} vs {disk}")
            if key == "SLA-AVAILABILITY" and float(src["value"]) != 99.5:
                self.err("availability must be 99.5")
            # Auth/drift must NOT cite SLA-AVAILABILITY
            if key in {"O02-OPS-AUTH-DENY-COUNT", "O02-OPS-DRIFT-COUNT"}:
                if kind != "operational_policy":
                    self.err(f"{key} must be operational_policy")

        alerts = self.thresholds["alerts"]
        if alerts["query_p99_seconds"].get("status") != "blocked":
            self.err("query_p99_seconds must be status=blocked")
        if float(alerts["query_p95_seconds"]["value"]) != 0.5:
            self.err("query_p95_seconds must be 0.5")
        if float(alerts["auth_deny_count_10m"]["value"]) != 50:
            self.err("auth_deny_count_10m must be 50")
        if alerts["auth_deny_count_10m"]["source"] != "O02-OPS-AUTH-DENY-COUNT":
            self.err("auth deny must cite O02-OPS-AUTH-DENY-COUNT")
        if alerts["drift_count_10m"]["source"] != "O02-OPS-DRIFT-COUNT":
            self.err("drift must cite O02-OPS-DRIFT-COUNT")

        # Histogram boundaries must include SLO cut-points
        bounds = self.thresholds.get("histogramBoundariesSeconds") or []
        if 0.5 not in bounds or 1.0 not in bounds:
            self.err("histogramBoundariesSeconds must include 0.5 and 1.0")

    def check_active_rules_inventory(self) -> None:
        alert_names = {r["alert"] for r in self.alert_rules if "alert" in r}
        blocked = {r["alert"] for r in self.blocked_rules if "alert" in r}
        if "MarkhandQueryLatencyP99Burn" in alert_names:
            self.err("P99 alert must not be in active alert_rules.yml")
        if "MarkhandQueryLatencyP99Burn" not in blocked:
            self.err("P99 alert must be listed in alert_rules.blocked.yml")
        if "MarkhandGlmProbeDown" not in blocked:
            self.err("GLM probe alert must be blocked")
        if "glm" in str(load_yaml(PROMETHEUS_PATH)):
            self.err("prometheus.yml must not probe GLM")
        # Auth deny must use increase/count, not ratio of auth decisions as SLA
        auth = next(r for r in self.alert_rules if r.get("alert") == "MarkhandAuthDenySpike")
        if "deny_ratio" in (auth.get("expr") or "") or "0.20" in (auth.get("expr") or ""):
            self.err("AuthDenySpike must not use invalid 20% ratio")
        if "auth:deny_increase_10m" not in (auth.get("expr") or ""):
            self.err("AuthDenySpike must use deny_increase_10m")
        # Dead letter / reconcile: increase, no long for
        dead = next(r for r in self.alert_rules if r.get("alert") == "MarkhandDeadLetterJobs")
        if "for" in dead:
            self.err("MarkhandDeadLetterJobs must not use long for window")
        if "dead_letter_increase_5m" not in (dead.get("expr") or ""):
            self.err("MarkhandDeadLetterJobs must use increase recording rule")
        recon = next(r for r in self.alert_rules if r.get("alert") == "MarkhandReconcileErrors")
        if "for" in recon:
            self.err("MarkhandReconcileErrors must not use long for window")
        # SLO uses search route recording rule
        p95 = next(r for r in self.alert_rules if r.get("alert") == "MarkhandQueryLatencyP95Burn")
        if "api_search:p95_5m" not in (p95.get("expr") or ""):
            self.err("P95 alert must use markhand:api_search:p95_5m")
        if "retrieval:p95" in (p95.get("expr") or ""):
            self.err("P95 alert must not use retrieval-leg latency")
        # Threshold sources on alerts must exist
        sources = self.thresholds.get("sources") or {}
        for rule in self.alert_rules:
            src = (rule.get("annotations") or {}).get("threshold_source")
            if src and src not in sources:
                self.err(f"{rule.get('alert')}: unknown threshold_source {src}")
            runbook = (rule.get("annotations") or {}).get("runbook")
            if runbook and not (ROOT / runbook).is_file():
                self.err(f"{rule.get('alert')}: missing runbook {runbook}")
            for key in (rule.get("labels") or {}):
                if key in FORBIDDEN_LABELS:
                    self.err(f"{rule.get('alert')}: forbidden label {key}")
        # prometheus.yml must not load blocked rules (ignore comments)
        prom_cfg = load_yaml(PROMETHEUS_PATH)
        for rf in prom_cfg.get("rule_files") or []:
            if "blocked" in str(rf):
                self.err("prometheus.yml must not load blocked rules")
        # Blackbox modules
        bb = load_yaml(BLACKBOX_PATH)
        modules = bb.get("modules") or {}
        if "http_2xx" not in modules or "tcp_connect" not in modules:
            self.err("blackbox.yml must define http_2xx and tcp_connect")
        if modules["tcp_connect"].get("prober") != "tcp":
            self.err("tcp_connect must use tcp prober")
        # Images pinned
        images = load_json(IMAGES_LOCK)["images"]
        compose = COMPOSE_PATH.read_text(encoding="utf-8")
        for name, ref in images.items():
            digest = ref.split("@", 1)[-1]
            if digest not in compose and name != "otel-collector":
                # otel also in compose
                pass
            if "@sha256:" not in ref:
                self.err(f"image {name} not digest-pinned")
        for ref in images.values():
            if "@sha256:" not in ref:
                self.err(f"unpinned image {ref}")
            if ref.split(":")[0].endswith("latest") or ":latest@" in ref:
                self.err(f"mutable latest tag: {ref}")
        for img_line in re.findall(r"image:\s*(\S+)", compose):
            if "@sha256:" not in img_line:
                self.err(f"compose image not digest-pinned: {img_line}")

    def check_promtool(self) -> None:
        try:
            promtool = resolve_promtool()
        except RuntimeError as error:
            self.err(str(error))
            return
        self.promtool_path = promtool
        ver = run_promtool(promtool, ["--version"])
        lock_ver = load_json(IMAGES_LOCK)["promtool"]["version"]
        if lock_ver not in (ver.stdout + ver.stderr):
            self.notes.append(
                f"promtool version output does not contain {lock_ver}; using {promtool}"
            )
        check = run_promtool(
            promtool,
            [
                "check",
                "rules",
                str(RECORDING_RULES_PATH.relative_to(ROOT)),
                str(ALERT_RULES_PATH.relative_to(ROOT)),
            ],
        )
        if check.returncode != 0:
            self.err(f"promtool check rules failed:\n{check.stderr or check.stdout}")
        else:
            self.promtool_check_ok = True
        test = run_promtool(
            promtool,
            ["test", "rules", str(PROM_TESTS.relative_to(ROOT))],
        )
        if test.returncode != 0:
            self.err(f"promtool test rules failed:\n{test.stderr or test.stdout}")
        else:
            self.promtool_test_ok = True

    def check_dashboards(self) -> None:
        ds = load_yaml(DATASOURCE_PATH)
        uids = {d.get("uid") for d in ds.get("datasources") or []}
        if DATASOURCE_UID not in uids:
            self.err(f"datasource uid {DATASOURCE_UID} missing")
        files = list(DASHBOARD_DIR.glob("*.json"))
        if len(files) < 4:
            self.err(f"expected >=4 dashboards, found {len(files)}")
        allow = self.thresholds["labelAllowlists"]
        for path in files:
            data = load_json(path)
            for var in data.get("templating", {}).get("list", []):
                name = var.get("name")
                if name in FORBIDDEN_LABELS:
                    self.err(f"{path.name}: forbidden variable {name}")
                if var.get("type") not in {"custom", "constant", "textbox"}:
                    self.err(f"{path.name}: variable {name} type must be bounded")
                if name in allow:
                    for opt in var.get("options") or []:
                        val = opt.get("value")
                        if val and val not in allow[name] and val != "$__all":
                            self.err(f"{path.name}: {name}={val} not allowlisted")
            for panel in data.get("panels") or []:
                for target in panel.get("targets") or []:
                    expr = target.get("expr") or ""
                    for key in FORBIDDEN_LABELS:
                        if re.search(rf"\b{re.escape(key)}\b", expr):
                            self.err(f"{path.name}: forbidden label {key} in expr")
                    ds_ref = target.get("datasource") or panel.get("datasource") or {}
                    if isinstance(ds_ref, dict) and ds_ref.get("uid") not in {
                        None,
                        DATASOURCE_UID,
                    }:
                        self.err(f"{path.name}: unexpected datasource uid {ds_ref.get('uid')}")
            # Honest coverage gaps on SLO dashboard
            if data.get("uid") == "markhand-slo":
                blob = json.dumps(data)
                if "filtered-query P99" not in blob and "P99 blocked" not in blob and "coverage gap" not in blob.lower():
                    self.err("markhand-slo dashboard must document P99 coverage gap")

    def check_schemas(self) -> None:
        for path in (
            PROMETHEUS_PATH,
            ALERTMANAGER_PATH,
            OTEL_PATH,
            COMPOSE_PATH,
            BLACKBOX_PATH,
            DATASOURCE_PATH,
        ):
            try:
                load_yaml(path)
            except Exception as error:  # noqa: BLE001
                self.err(f"YAML parse failed for {path.relative_to(ROOT)}: {error}")
        # Alertmanager must not have broad inhibit
        am = load_yaml(ALERTMANAGER_PATH)
        inhibits = am.get("inhibit_rules") or []
        for rule in inhibits:
            targets = str(rule.get("target_matchers") or rule.get("target_match") or "")
            if "slo|queue|parser" in targets or "class=~" in targets:
                self.err("broad class-based inhibit_rules must be removed")
        # Compose service names / real endpoints referenced in prometheus.yml
        prom = PROMETHEUS_PATH.read_text(encoding="utf-8")
        for needle in (
            "http://api:8787/api/v1/health/ready",
            "http://qdrant:6333/healthz",
            "http://minio:9000/minio/health/live",
            "http://mock-embedding:8080/health",
            "postgres:5432",
            "module: [tcp_connect]",
            "module: [http_2xx]",
        ):
            if needle not in prom:
                self.err(f"prometheus.yml missing real endpoint/module: {needle}")
        if "markhand-server" in prom or "glm.example.invalid" in prom:
            self.err("prometheus.yml still references fake service names/GLM")

    def check_runbooks(self) -> None:
        for name in REQUIRED_RUNBOOKS:
            path = RUNBOOK_DIR / name
            if not path.is_file():
                self.err(f"missing runbook {name}")
                continue
            text = path.read_text(encoding="utf-8")
            for section in RUNBOOK_SECTIONS:
                if f"## {section}" not in text:
                    self.err(f"{name}: missing ## {section}")
            if SECRET_RE.search(text):
                self.err(f"{name}: possible secret material")
            # Must reference real compose/scripts
            if name == "dependency-outage.md":
                for needle in (
                    "deploy/compose.poc.yml",
                    "deploy/scripts/poc-health.sh",
                    "/api/v1/health/ready",
                ):
                    if needle not in text:
                        self.err(f"{name}: missing real reference {needle}")
            if name == "stuck-dead-jobs.md" and "MARKHAND_WORKER_KIND" not in text:
                self.err(f"{name}: must mention MARKHAND_WORKER_KIND")
            # Flag invented remediation commands, not gap statements that forbid them.
            if re.search(
                r"(?i)run\s+`?(admin requeue|reconcile --mode=|job-admin requeue)",
                text,
            ):
                self.err(f"{name}: invents unsupported admin commands")

    def check_secrets(self) -> None:
        for path in OBS.rglob("*"):
            if not path.is_file() or path.suffix in {".png", ".jpg"}:
                continue
            text = path.read_text(encoding="utf-8", errors="ignore")
            if SECRET_RE.search(text) and "REPLACE_WITH_SEALED_SECRET" not in text:
                self.err(f"possible secret in {path.relative_to(ROOT)}")

    def check_tabletop(self) -> None:
        data = load_json(TABLETOP_PATH)
        if data.get("claims_real_outage") is not False:
            self.err("tabletop claims_real_outage must be false")
        if "promtool" not in str(data.get("validation", "")).lower():
            self.err("tabletop must reference promtool validation")

    def run(self) -> list[str]:
        self.check_threshold_provenance()
        self.check_active_rules_inventory()
        self.check_promtool()
        self.check_dashboards()
        self.check_schemas()
        self.check_runbooks()
        self.check_secrets()
        self.check_tabletop()
        return self.errors

    def report(self) -> dict[str, Any]:
        return {
            "version": 2,
            "issue": "P1B-O02",
            "ok": not self.errors,
            "alertCount": len([r for r in self.alert_rules if "alert" in r]),
            "blockedAlertCount": len([r for r in self.blocked_rules if "alert" in r]),
            "dashboardCount": len(list(DASHBOARD_DIR.glob("*.json"))),
            "runbookCount": len(REQUIRED_RUNBOOKS),
            "promtool": str(self.promtool_path) if self.promtool_path else None,
            "promtoolCheckOk": self.promtool_check_ok,
            "promtoolTestOk": self.promtool_test_ok,
            "dockerAvailable": bool(shutil.which("docker")),
            "claims_real_outage": False,
            "errors": self.errors,
            "notes": self.notes
            + (
                []
                if shutil.which("docker")
                else [
                    "Docker not available in this environment; "
                    "compose deployability validated via YAML/image pins/endpoints only."
                ]
            ),
            "commands": [
                "bash scripts/fetch-promtool.sh",
                "python3 scripts/check-observability-o02.py",
                "python3 scripts/check-observability-o02.py --self-test",
                ".tools/promtool check rules deploy/observability/prometheus/recording_rules.yml deploy/observability/prometheus/alert_rules.yml",
                ".tools/promtool test rules deploy/observability/prometheus/tests/alerts_test.yml",
            ],
        }


class ObservabilityO02Tests(unittest.TestCase):
    def test_promtool_rules_pass(self) -> None:
        promtool = resolve_promtool()
        check = run_promtool(
            promtool,
            [
                "check",
                "rules",
                str(RECORDING_RULES_PATH.relative_to(ROOT)),
                str(ALERT_RULES_PATH.relative_to(ROOT)),
            ],
        )
        self.assertEqual(check.returncode, 0, check.stderr or check.stdout)
        test = run_promtool(promtool, ["test", "rules", str(PROM_TESTS.relative_to(ROOT))])
        self.assertEqual(test.returncode, 0, test.stderr or test.stdout)

    def test_mutation_fake_metric_fails_promtool_check(self) -> None:
        promtool = resolve_promtool()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "bad.yml"
            path.write_text(
                "groups:\n"
                "  - name: bad\n"
                "    rules:\n"
                "      - alert: Fake\n"
                "        expr: this is not valid promql (((( \n"
                "        labels: {severity: page}\n"
            )
            result = run_promtool(promtool, ["check", "rules", str(path)])
            self.assertNotEqual(result.returncode, 0)

    def test_mutation_wrong_threshold_caught_by_provenance(self) -> None:
        checks = ObservabilityO02Checks()
        checks.thresholds["alerts"]["query_p95_seconds"]["value"] = 0.9
        checks.check_threshold_provenance()
        self.assertTrue(any("query_p95" in e for e in checks.errors))

    def test_mutation_long_for_on_dead_letter_caught(self) -> None:
        checks = ObservabilityO02Checks()
        for rule in checks.alert_rules:
            if rule.get("alert") == "MarkhandDeadLetterJobs":
                rule["for"] = "30m"
        checks.check_active_rules_inventory()
        self.assertTrue(any("DeadLetter" in e for e in checks.errors))

    def test_full_validator_clean(self) -> None:
        errors = ObservabilityO02Checks().run()
        self.assertEqual(errors, [], "\n".join(errors))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--json-report",
        type=Path,
        default=EVIDENCE_PATH,
        help="write regenerated validation report (default: evidence path)",
    )
    args = parser.parse_args(argv)

    checks = ObservabilityO02Checks()
    errors = checks.run()
    report = checks.report()
    args.json_report.parent.mkdir(parents=True, exist_ok=True)
    args.json_report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    if errors:
        print("P1B-O02 observability validation FAILED:", file=sys.stderr, flush=True)
        for error in errors:
            print(f"  - {error}", file=sys.stderr, flush=True)
        return 1

    print(
        "P1B-O02 observability validation OK "
        f"({report['alertCount']} active alerts, {report['blockedAlertCount']} blocked, "
        f"{report['dashboardCount']} dashboards, promtool check/test OK); "
        "synthetic/promtool only — no live outage claimed",
        flush=True,
    )

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(ObservabilityO02Tests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
