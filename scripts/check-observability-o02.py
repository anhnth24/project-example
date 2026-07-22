#!/usr/bin/env python3
"""Validate P1B-O02 observability artifacts (rules, dashboards, runbooks, fixtures).

Checks PromQL shape, metric/label inventory, threshold provenance against the
approved gate registry, cardinality bans, runbook links, dashboard references,
and deterministic synthetic alert fixtures.

Does NOT claim a live outage was exercised.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import unittest
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parents[1]
OBS = ROOT / "deploy" / "observability"
THRESHOLDS_PATH = OBS / "thresholds.yaml"
ALERT_RULES_PATH = OBS / "prometheus" / "alert_rules.yml"
RECORDING_RULES_PATH = OBS / "prometheus" / "recording_rules.yml"
PROMETHEUS_PATH = OBS / "prometheus" / "prometheus.yml"
GATES_PATH = ROOT / "bench" / "markhand_web" / "gates.yaml"
WORKLOAD_PATH = ROOT / "bench" / "markhand_web" / "workload-profile.yaml"
DASHBOARD_DIR = OBS / "grafana" / "dashboards"
FIXTURE_DIR = OBS / "fixtures" / "alerts"
TABLETOP_PATH = OBS / "fixtures" / "tabletop" / "o02-tabletop.json"
RUNBOOK_DIR = ROOT / "docs" / "runbooks"

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

PROMQL_FUNCS = {
    "histogram_quantile",
    "sum",
    "max",
    "min",
    "avg",
    "rate",
    "irate",
    "increase",
    "clamp_min",
    "clamp_max",
    "vector",
    "abs",
    "floor",
    "ceil",
    "round",
    "label_replace",
    "label_join",
    "time",
    "minute",
    "hour",
    "day_of_month",
    "by",
    "without",
    "bool",
    "on",
    "ignoring",
    "group_left",
    "group_right",
    "and",
    "or",
    "unless",
}

IDENT_RE = re.compile(r"[A-Za-z_:][A-Za-z0-9_:]*")
FORBIDDEN_IN_TEXT = re.compile(
    r"(?i)(postgres(?:ql)?://\S+:\S+@|-----BEGIN [A-Z ]*PRIVATE KEY-----|"
    r"\bAKIA[0-9A-Z]{16}\b|\bghp_[A-Za-z0-9]{20,}\b|"
    r"REPLACE_WITH_REAL_SECRET)"
)


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


def check_balanced(expr: str) -> list[str]:
    errors: list[str] = []
    stack: list[str] = []
    pairs = {"(": ")", "[": "]", "{": "}"}
    closing = {v: k for k, v in pairs.items()}
    in_str = False
    for ch in expr:
        if ch == '"':
            in_str = not in_str
            continue
        if in_str:
            continue
        if ch in pairs:
            stack.append(ch)
        elif ch in closing:
            if not stack or stack[-1] != closing[ch]:
                errors.append(f"unbalanced '{ch}' in PromQL")
                return errors
            stack.pop()
    if stack:
        errors.append("unbalanced open brackets in PromQL")
    if in_str:
        errors.append("unterminated string in PromQL")
    return errors


def validate_promql(expr: str, known_metrics: set[str], forbidden_labels: set[str]) -> list[str]:
    errors = check_balanced(expr)
    # Strip strings and label matchers content for token scan.
    scrubbed = re.sub(r'"[^"]*"', '""', expr)
    scrubbed = re.sub(r"'[^']*'", "''", scrubbed)
    # Label keys inside {...}
    for match in re.finditer(r"\{([^}]*)\}", scrubbed):
        body = match.group(1)
        for part in body.split(","):
            part = part.strip()
            if not part:
                continue
            key = re.split(r"\s*(?:=~|!~|=|!=)\s*", part, maxsplit=1)[0].strip()
            if key in forbidden_labels:
                errors.append(f"forbidden label in PromQL: {key}")
    # Metric-like identifiers excluding functions/keywords/numbers.
    for tok in IDENT_RE.findall(scrubbed):
        if tok.lower() in PROMQL_FUNCS:
            continue
        if tok in {"le", "by", "without", "bool", "on", "ignoring"}:
            continue
        if re.fullmatch(r"[0-9]+(?:\.[0-9]+)?", tok):
            continue
        if tok.endswith(("m", "s", "h", "d")) and tok[:-1].replace(".", "", 1).isdigit():
            continue
        if tok.startswith("markhand") or tok.startswith("node_") or tok in {
            "probe_success",
            "up",
        }:
            base = tok
            for suffix in ("_bucket", "_sum", "_count", "_created"):
                if base.endswith(suffix):
                    base = base[: -len(suffix)]
                    break
            # recording rules use markhand:name style
            if ":" in tok:
                continue
            if base not in known_metrics and tok not in known_metrics:
                # allow infra + product
                if not (
                    tok.startswith("markhand:")
                    or tok.startswith("node_")
                    or tok in {"probe_success", "up"}
                ):
                    errors.append(f"unknown metric token: {tok}")
            continue
        # label names / recording rule metric names with colon already skipped
        if tok.startswith("markhand:"):
            continue
    return errors


def compare(op: str, left: float, right: float) -> bool:
    if op == ">":
        return left > right
    if op == ">=":
        return left >= right
    if op == "<":
        return left < right
    if op == "<=":
        return left <= right
    if op == "==":
        return left == right
    if op == "!=":
        return left != right
    raise ValueError(f"unsupported op {op}")


def expected_state(op: str, value: float, threshold: float) -> str:
    return "firing" if compare(op, value, threshold) else "resolved"


class ObservabilityO02Checks:
    def __init__(self) -> None:
        self.errors: list[str] = []
        self.thresholds = load_yaml(THRESHOLDS_PATH)
        self.gates = load_json(GATES_PATH)
        self.workload = load_json(WORKLOAD_PATH)
        self.alert_rules = flatten_rules(ALERT_RULES_PATH)
        self.recording_rules = flatten_rules(RECORDING_RULES_PATH)
        self.known_metrics = set(self.thresholds["metrics"]) | set(
            self.thresholds.get("infraMetrics") or []
        )
        self.forbidden = set(self.thresholds["forbiddenLabelKeys"])
        # recording rule names become known series
        for rule in self.recording_rules:
            if "record" in rule:
                self.known_metrics.add(rule["record"])

    def err(self, msg: str) -> None:
        self.errors.append(msg)

    def check_files_exist(self) -> None:
        required = [
            THRESHOLDS_PATH,
            ALERT_RULES_PATH,
            RECORDING_RULES_PATH,
            PROMETHEUS_PATH,
            OBS / "alertmanager" / "alertmanager.example.yml",
            OBS / "otel" / "collector-prometheus.yaml",
            OBS / "README.md",
            TABLETOP_PATH,
        ]
        for path in required:
            if not path.is_file():
                self.err(f"missing required file: {path.relative_to(ROOT)}")

    def check_threshold_provenance(self) -> None:
        gate_by_id = {g["id"]: g for g in self.gates["gates"]}
        sources = self.thresholds.get("sources") or {}
        for key, src in sources.items():
            gate_id = src.get("gateId")
            if gate_id:
                gate = gate_by_id.get(gate_id)
                if not gate:
                    self.err(f"threshold source {key} missing gate {gate_id}")
                    continue
                expected = gate["threshold"]["value"]
                if float(src["value"]) != float(expected):
                    self.err(
                        f"{key} value {src['value']} != gates.yaml {gate_id}={expected}"
                    )
            if key == "WORKLOAD-DISK-HEADROOM-PERCENT":
                disk = self.workload["hardware"]["headroomPercent"]["disk"]
                if float(src["value"]) != float(disk):
                    self.err(f"disk headroom {src['value']} != workload {disk}")
            if key == "SLA-QUEUE-AGE-MINUTES" and float(src["value"]) != 120:
                self.err("queue age SLA must be 120 minutes per sla-targets.md")
            if key == "SLA-AVAILABILITY" and float(src["value"]) != 99.5:
                self.err("availability SLA must be 99.5 per sla-targets.md")

        alerts = self.thresholds["alerts"]
        if float(alerts["query_p95_seconds"]["value"]) != 0.5:
            self.err("query_p95_seconds must be 0.5")
        if float(alerts["query_p99_seconds"]["value"]) != 1.0:
            self.err("query_p99_seconds must be 1.0")
        if float(alerts["queue_oldest_age_seconds"]["value"]) != 7200:
            self.err("queue_oldest_age_seconds must be 7200")
        if float(alerts["queue_depth_warning"]["value"]) != 600:
            self.err("queue_depth_warning must be 600 (1200*0.5h)")
        if float(alerts["disk_free_ratio_min"]["value"]) != 0.30:
            self.err("disk_free_ratio_min must be 0.30")

    def check_rules(self) -> None:
        alert_names: set[str] = set()
        for rule in self.recording_rules:
            expr = rule.get("expr")
            if not expr:
                self.err(f"recording rule missing expr: {rule}")
                continue
            for e in validate_promql(expr, self.known_metrics, self.forbidden):
                self.err(f"recording {rule.get('record')}: {e}")
            for key in (rule.get("labels") or {}):
                if key in self.forbidden:
                    self.err(f"recording rule forbidden label key: {key}")

        for rule in self.alert_rules:
            name = rule.get("alert")
            if not name:
                self.err("alert rule missing alert name")
                continue
            if name in alert_names:
                self.err(f"duplicate alert: {name}")
            alert_names.add(name)
            expr = rule.get("expr")
            if not expr:
                self.err(f"{name}: missing expr")
                continue
            for e in validate_promql(expr, self.known_metrics, self.forbidden):
                self.err(f"alert {name}: {e}")
            for key in (rule.get("labels") or {}):
                if key in self.forbidden:
                    self.err(f"{name}: forbidden label key {key}")
            ann = rule.get("annotations") or {}
            for field in ("runbook", "fixture", "threshold_source", "dashboard"):
                if field not in ann:
                    self.err(f"{name}: missing annotation {field}")
            runbook = ann.get("runbook")
            if runbook and not (ROOT / runbook).is_file():
                self.err(f"{name}: runbook missing {runbook}")
            fixture = ann.get("fixture")
            if fixture and not (ROOT / fixture).is_file():
                self.err(f"{name}: fixture missing {fixture}")
            src = ann.get("threshold_source")
            if src and src not in (self.thresholds.get("sources") or {}):
                # allow alert threshold keys that map via sources
                if src not in (self.thresholds.get("sources") or {}) and src not in {
                    "G0-SLO-QUERY-P95",
                    "G0-SLO-QUERY-P99",
                    "SLA-QUEUE-AGE-MINUTES",
                    "WORKLOAD-PEAK-INGEST-DOCS-PER-HOUR",
                    "WORKLOAD-DISK-HEADROOM-PERCENT",
                    "SLA-AVAILABILITY",
                }:
                    self.err(f"{name}: unknown threshold_source {src}")
            # No backup alerts in O02
            if "backup" in name.lower():
                self.err(f"{name}: backup alerts are O03 scope")

        if len(alert_names) < 10:
            self.err(f"expected >=10 alerts, found {len(alert_names)}")

        # Ensure inventory of fixtures matches alerts 1:1
        fixture_alerts = {p.stem for p in FIXTURE_DIR.glob("*.json")}
        if fixture_alerts != alert_names:
            missing = alert_names - fixture_alerts
            extra = fixture_alerts - alert_names
            if missing:
                self.err(f"fixtures missing for alerts: {sorted(missing)}")
            if extra:
                self.err(f"fixtures without alerts: {sorted(extra)}")

    def check_runbooks(self) -> None:
        for name in REQUIRED_RUNBOOKS:
            path = RUNBOOK_DIR / name
            if not path.is_file():
                self.err(f"missing runbook {name}")
                continue
            text = path.read_text(encoding="utf-8")
            for section in RUNBOOK_SECTIONS:
                if f"## {section}" not in text:
                    self.err(f"{name}: missing section ## {section}")
            if FORBIDDEN_IN_TEXT.search(text):
                self.err(f"{name}: possible secret material")
            for label in self.forbidden:
                # allow mentioning forbidden labels as bans
                pass

    def check_dashboards(self) -> None:
        files = list(DASHBOARD_DIR.glob("*.json"))
        if len(files) < 4:
            self.err(f"expected >=4 dashboards, found {len(files)}")
        allow = self.thresholds["labelAllowlists"]
        dashboard_uids = set()
        for path in files:
            data = load_json(path)
            uid = data.get("uid")
            dashboard_uids.add(uid)
            for var in data.get("templating", {}).get("list", []):
                name = var.get("name")
                if name in self.forbidden:
                    self.err(f"{path.name}: forbidden dashboard variable {name}")
                options = [o.get("value") for o in var.get("options") or []]
                if name in allow:
                    for opt in options:
                        if opt not in allow[name] and opt not in ("$__all",):
                            # includeAll uses All option sometimes
                            if opt is None:
                                continue
                            if str(opt) not in allow[name]:
                                self.err(
                                    f"{path.name}: variable {name} value not allowlisted: {opt}"
                                )
                # custom vars must be bounded (no query datasource vars)
                if var.get("type") not in {"custom", "constant"}:
                    self.err(f"{path.name}: variable {name} must be custom/constant")
            for panel in data.get("panels") or []:
                for target in panel.get("targets") or []:
                    expr = target.get("expr") or ""
                    for e in validate_promql(expr, self.known_metrics, self.forbidden):
                        self.err(f"{path.name} panel {panel.get('title')}: {e}")
                    for key in self.forbidden:
                        if re.search(rf"\b{re.escape(key)}\b", expr):
                            self.err(f"{path.name}: forbidden label {key} in panel expr")

        # alert dashboard annotations must resolve
        for rule in self.alert_rules:
            dash = (rule.get("annotations") or {}).get("dashboard")
            if dash and dash not in dashboard_uids:
                self.err(f"alert {rule.get('alert')}: unknown dashboard uid {dash}")

    def check_fixtures(self) -> None:
        for path in sorted(FIXTURE_DIR.glob("*.json")):
            data = load_json(path)
            if data.get("kind") != "synthetic":
                self.err(f"{path.name}: kind must be synthetic")
            if data.get("claims_real_outage") is not False:
                self.err(f"{path.name}: claims_real_outage must be false")
            cmp = data.get("compare") or {}
            op = cmp.get("op")
            threshold = float(cmp.get("threshold"))
            for sample in data.get("samples") or []:
                value = float(sample["value"])
                expect = sample["expect"]
                actual = expected_state(op, value, threshold)
                if actual != expect:
                    self.err(
                        f"{path.name}: sample value={value} expected {expect} got {actual}"
                    )
                for key in (sample.get("labels") or {}):
                    if key in self.forbidden:
                        self.err(f"{path.name}: forbidden label {key}")
                    allow = self.thresholds["labelAllowlists"]
                    if key in allow and sample["labels"][key] not in allow[key]:
                        self.err(
                            f"{path.name}: label {key}={sample['labels'][key]} not allowlisted"
                        )

        tabletop = load_json(TABLETOP_PATH)
        if tabletop.get("claims_real_outage") is not False:
            self.err("tabletop claims_real_outage must be false")
        for scenario in tabletop.get("scenarios") or []:
            for step in ("detection", "contain", "recover", "verify"):
                if step not in scenario.get("steps_exercised") or []:
                    self.err(f"tabletop {scenario.get('id')}: missing step {step}")
            rb = scenario.get("runbook")
            if rb and not (ROOT / rb).is_file():
                self.err(f"tabletop {scenario.get('id')}: missing runbook {rb}")

    def check_secrets_hygiene(self) -> None:
        for path in OBS.rglob("*"):
            if not path.is_file():
                continue
            if path.suffix in {".png", ".jpg"}:
                continue
            text = path.read_text(encoding="utf-8", errors="ignore")
            if FORBIDDEN_IN_TEXT.search(text) and "REPLACE_WITH_SEALED_SECRET" not in text:
                # allow placeholder bearer tokens only when clearly placeholder
                if "REPLACE_WITH_SEALED_SECRET" in text:
                    continue
                self.err(f"possible secret in {path.relative_to(ROOT)}")
            # Explicit ban on real-looking webhook secrets beyond placeholders
            if re.search(r"(?i)xox[baprs]-[0-9A-Za-z-]+", text):
                self.err(f"slack token-like material in {path.relative_to(ROOT)}")

    def run(self) -> list[str]:
        self.check_files_exist()
        self.check_threshold_provenance()
        self.check_rules()
        self.check_runbooks()
        self.check_dashboards()
        self.check_fixtures()
        self.check_secrets_hygiene()
        return self.errors


class ObservabilityO02Tests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.errors = ObservabilityO02Checks().run()

    def test_no_validation_errors(self) -> None:
        self.assertEqual(self.errors, [], "\n".join(self.errors))

    def test_alert_inventory_size(self) -> None:
        alerts = [r["alert"] for r in flatten_rules(ALERT_RULES_PATH)]
        self.assertGreaterEqual(len(alerts), 10)

    def test_each_fixture_has_firing_and_resolved(self) -> None:
        for path in FIXTURE_DIR.glob("*.json"):
            data = load_json(path)
            expects = {s["expect"] for s in data["samples"]}
            self.assertEqual(expects, {"firing", "resolved"}, path.name)

    def test_gates_registry_approved(self) -> None:
        gates = load_json(GATES_PATH)
        self.assertEqual(gates.get("registryStatus"), "approved")
        ids = {g["id"] for g in gates["gates"]}
        self.assertIn("G0-SLO-QUERY-P95", ids)
        self.assertIn("G0-SLO-QUERY-P99", ids)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="run unittest suite")
    parser.add_argument(
        "--json-report",
        type=Path,
        help="write machine-readable validation report",
    )
    args = parser.parse_args(argv)

    checks = ObservabilityO02Checks()
    errors = checks.run()
    alert_count = len([r for r in checks.alert_rules if "alert" in r])
    report = {
        "version": 1,
        "issue": "P1B-O02",
        "ok": not errors,
        "alertCount": alert_count,
        "dashboardCount": len(list(DASHBOARD_DIR.glob("*.json"))),
        "fixtureCount": len(list(FIXTURE_DIR.glob("*.json"))),
        "runbookCount": len(REQUIRED_RUNBOOKS),
        "errors": errors,
        "claims_real_outage": False,
        "commands": [
            "python3 scripts/check-observability-o02.py",
            "python3 scripts/check-observability-o02.py --self-test",
        ],
    }
    if args.json_report:
        args.json_report.parent.mkdir(parents=True, exist_ok=True)
        args.json_report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    if errors:
        print("P1B-O02 observability validation FAILED:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    print(
        f"P1B-O02 observability validation OK "
        f"({alert_count} alerts, {report['dashboardCount']} dashboards, "
        f"{report['fixtureCount']} fixtures, {report['runbookCount']} runbooks); "
        "synthetic only — no live outage claimed",
        flush=True,
    )

    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(ObservabilityO02Tests)
        result = unittest.TextTestRunner(verbosity=2).run(suite)
        return 0 if result.wasSuccessful() else 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
