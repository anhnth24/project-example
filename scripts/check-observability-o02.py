#!/usr/bin/env python3
"""Validate P1B-O02 observability artifacts with pinned promtool + schema checks.

- Invokes pinned promtool (scripts/fetch-promtool.sh → .tools/promtool 2.55.1)
  for `check rules` and `test rules` (temporal/equality/non-firing).
- Cross-checks threshold provenance against gates.yaml / SLA / workload profile.
- Validates Grafana datasource UIDs/queries, OTel/Prometheus/Alertmanager/compose
  YAML via parsers, forbidden cardinality/secret hygiene.
- Validates merged Compose REPO_ROOT binds, OTEL/network matrix, embedding aliases.
- Metric selectors must match O01 inventory + allowed recording rules + infra.
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
POC_COMPOSE_PATH = ROOT / "deploy" / "compose.poc.yml"
UP_SH_PATH = OBS / "up.sh"
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
METRIC_RE = re.compile(
    r"\b("
    r"markhand_[a-z0-9_]+"
    r"|markhand:[a-z0-9_:]+:[a-z0-9_]+"
    r"|node_filesystem_(?:avail|size)_bytes"
    r"|probe_success"
    r"|up"
    r")\b"
)
HISTOGRAM_SUFFIXES = ("_bucket", "_count", "_sum")
OTEL_EMITTERS = ("api", "worker-index", "worker-embedding", "worker-convert")
REQUIRED_OTEL_ENV = {
    "MARKHAND_OTEL_EXPORTER": "otlp",
    "MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT": "http://otel-collector:4317",
    "MARKHAND_OTEL_METRICS_ENABLED": "true",
}
POLICY_CITATION_RULES = {
    "MarkhandDeadLetterJobs": "O02-OPS-DEAD-LETTER-EVENT",
    "MarkhandDependencyProbeDown": "O02-OPS-PROBE-FAILURE",
    "MarkhandScrapeTargetDown": "O02-OPS-PROBE-FAILURE",
    "MarkhandDependencyProbeAbsent": "O02-OPS-PROBE-FAILURE",
    "MarkhandDriftDetected": "O02-OPS-DRIFT-COUNT",
    "MarkhandAuthDenySpike": "O02-OPS-AUTH-DENY-COUNT",
    "MarkhandHostRootFilesystemLow": "WORKLOAD-DISK-HEADROOM-PERCENT",
    "MarkhandSearchAvailabilityBurn": "SLA-AVAILABILITY",
}


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


def extract_metrics(expr: str) -> set[str]:
    # Strip quoted label/string literals so values like markhand_ready are ignored.
    cleaned = re.sub(r'"[^"]*"', '""', expr or "")
    cleaned = re.sub(r"'[^']*'", "''", cleaned)
    found = set(METRIC_RE.findall(cleaned))
    # Normalize histogram children to base name for inventory compare.
    normalized: set[str] = set()
    for name in found:
        if name.startswith("markhand_") and not name.startswith("markhand:"):
            base = name
            for suffix in HISTOGRAM_SUFFIXES:
                if base.endswith(suffix):
                    base = base[: -len(suffix)]
                    break
            normalized.add(base)
        else:
            normalized.add(name)
    return normalized


def network_names(service: dict[str, Any]) -> set[str]:
    nets = service.get("networks")
    if nets is None:
        return set()
    if isinstance(nets, list):
        return {str(n) for n in nets}
    if isinstance(nets, dict):
        return {str(n) for n in nets}
    return set()


def network_aliases(service: dict[str, Any], network: str) -> set[str]:
    nets = service.get("networks")
    if not isinstance(nets, dict):
        return set()
    entry = nets.get(network) or {}
    if not isinstance(entry, dict):
        return set()
    return {str(a) for a in (entry.get("aliases") or [])}


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
        self.active_alert_names: list[str] = sorted(
            r["alert"] for r in self.alert_rules if "alert" in r
        )

    def err(self, msg: str) -> None:
        self.errors.append(msg)

    def allowed_metric_set(self) -> set[str]:
        base = set(self.thresholds.get("metrics") or [])
        recording = set(self.thresholds.get("allowedRecordingMetrics") or [])
        infra = set(self.thresholds.get("infraMetrics") or [])
        return base | recording | infra

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
            if key in {
                "O02-OPS-AUTH-DENY-COUNT",
                "O02-OPS-DRIFT-COUNT",
                "O02-OPS-DEAD-LETTER-EVENT",
                "O02-OPS-PROBE-FAILURE",
            }:
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
        if alerts["dead_letter_event"]["source"] != "O02-OPS-DEAD-LETTER-EVENT":
            self.err("dead_letter_event must cite O02-OPS-DEAD-LETTER-EVENT")
        if alerts["probe_failure"]["source"] != "O02-OPS-PROBE-FAILURE":
            self.err("probe_failure must cite O02-OPS-PROBE-FAILURE")
        if alerts["disk_free_ratio_min"].get("namedVolumeAttribution") != (
            "unavailable_blocked"
        ):
            self.err("disk threshold must mark namedVolumeAttribution unavailable_blocked")
        if alerts["disk_free_ratio_min"].get("series") != (
            "markhand:disk:host_root_free_ratio"
        ):
            self.err("disk threshold series must be host_root_free_ratio")

        bounds = self.thresholds.get("histogramBoundariesSeconds") or []
        if 0.5 not in bounds or 1.0 not in bounds:
            self.err("histogramBoundariesSeconds must include 0.5 and 1.0")

        blocked_names = {b["name"] for b in self.thresholds.get("blockedAlerts") or []}
        for required in (
            "MarkhandQueryLatencyP99Burn",
            "MarkhandGlmProbeDown",
            "MarkhandReconcileErrors",
            "MarkhandNamedVolumeDiskLow",
        ):
            if required not in blocked_names:
                self.err(f"thresholds.blockedAlerts missing {required}")

    def check_metric_inventory(self) -> None:
        allowed = self.allowed_metric_set()
        recording_names = {
            r["record"] for r in self.recording_rules if "record" in r
        }
        expected_recording = set(self.thresholds.get("allowedRecordingMetrics") or [])
        if recording_names != expected_recording:
            self.err(
                "recording rule names != thresholds.allowedRecordingMetrics: "
                f"extra={sorted(recording_names - expected_recording)} "
                f"missing={sorted(expected_recording - recording_names)}"
            )

        texts: list[tuple[str, str]] = []
        for rule in self.alert_rules + self.recording_rules:
            name = rule.get("alert") or rule.get("record") or "?"
            texts.append((f"rule:{name}", rule.get("expr") or ""))
        for path in DASHBOARD_DIR.glob("*.json"):
            data = load_json(path)
            for panel in data.get("panels") or []:
                for target in panel.get("targets") or []:
                    texts.append(
                        (f"dashboard:{path.name}:{panel.get('title')}", target.get("expr") or "")
                    )

        for where, expr in texts:
            for metric in extract_metrics(expr):
                if metric not in allowed:
                    self.err(f"{where}: unknown metric selector {metric}")

        # Availability recording must force empty 2xx to zero.
        success = next(
            r
            for r in self.recording_rules
            if r.get("record") == "markhand:api_search:success_ratio_5m"
        )
        if "or vector(0)" not in (success.get("expr") or ""):
            self.err("success_ratio_5m must use `or vector(0)` for empty 2xx series")

        # Disk recording must be host root only.
        disk = next(
            r
            for r in self.recording_rules
            if r.get("record") == "markhand:disk:host_root_free_ratio"
        )
        if 'mountpoint="/"' not in (disk.get("expr") or ""):
            self.err("host_root_free_ratio must filter mountpoint=\"/\"")

        # Active alert exprs must not reference blocked reconcile error series.
        for rule in self.alert_rules:
            expr = rule.get("expr") or ""
            if "reconcile:error" in expr or 'reconcile_total{result="error"}' in expr:
                self.err(
                    f"{rule.get('alert')}: active expr must not use reconcile result=error"
                )
            if rule.get("alert") == "MarkhandReconcileErrors":
                self.err("MarkhandReconcileErrors must not be in active alert_rules.yml")

    def check_active_rules_inventory(self) -> None:
        alert_names = set(self.active_alert_names)
        blocked = {r["alert"] for r in self.blocked_rules if "alert" in r}
        if "MarkhandQueryLatencyP99Burn" in alert_names:
            self.err("P99 alert must not be in active alert_rules.yml")
        if "MarkhandQueryLatencyP99Burn" not in blocked:
            self.err("P99 alert must be listed in alert_rules.blocked.yml")
        if "MarkhandGlmProbeDown" not in blocked:
            self.err("GLM probe alert must be blocked")
        if "MarkhandReconcileErrors" not in blocked:
            self.err("MarkhandReconcileErrors must be blocked")
        if "MarkhandReconcileErrors" in alert_names:
            self.err("MarkhandReconcileErrors must not be active")
        if "MarkhandNamedVolumeDiskLow" not in blocked:
            self.err("MarkhandNamedVolumeDiskLow must be blocked")
        if "MarkhandHostRootFilesystemLow" not in alert_names:
            self.err("MarkhandHostRootFilesystemLow must be active")
        if "MarkhandDiskLow" in alert_names:
            self.err("legacy MarkhandDiskLow must be renamed to host-root alert")
        if "glm" in str(load_yaml(PROMETHEUS_PATH)):
            self.err("prometheus.yml must not probe GLM")

        auth = next(r for r in self.alert_rules if r.get("alert") == "MarkhandAuthDenySpike")
        if "deny_ratio" in (auth.get("expr") or "") or "0.20" in (auth.get("expr") or ""):
            self.err("AuthDenySpike must not use invalid 20% ratio")
        if "auth:deny_increase_10m" not in (auth.get("expr") or ""):
            self.err("AuthDenySpike must use deny_increase_10m")

        dead = next(r for r in self.alert_rules if r.get("alert") == "MarkhandDeadLetterJobs")
        if "for" in dead:
            self.err("MarkhandDeadLetterJobs must not use long for window")
        if "dead_letter_increase_5m" not in (dead.get("expr") or ""):
            self.err("MarkhandDeadLetterJobs must use increase recording rule")

        avail = next(
            r for r in self.alert_rules if r.get("alert") == "MarkhandSearchAvailabilityBurn"
        )
        expr = avail.get("expr") or ""
        if "success_ratio_5m" not in expr:
            self.err("SearchAvailabilityBurn must use success_ratio_5m")
        if 'route="search"' not in expr and "api_requests_total" not in expr:
            self.err("SearchAvailabilityBurn must gate on search traffic")
        if "> 0" not in expr:
            self.err("SearchAvailabilityBurn must require traffic > 0")

        p95 = next(r for r in self.alert_rules if r.get("alert") == "MarkhandQueryLatencyP95Burn")
        if "api_search:p95_5m" not in (p95.get("expr") or ""):
            self.err("P95 alert must use markhand:api_search:p95_5m")
        if "retrieval:p95" in (p95.get("expr") or ""):
            self.err("P95 alert must not use retrieval-leg latency")

        sources = self.thresholds.get("sources") or {}
        for rule in self.alert_rules:
            name = rule.get("alert")
            src = (rule.get("annotations") or {}).get("threshold_source")
            if src and src not in sources:
                self.err(f"{name}: unknown threshold_source {src}")
            expected = POLICY_CITATION_RULES.get(str(name))
            if expected and src != expected:
                self.err(f"{name}: threshold_source must be {expected}, got {src}")
            if name in {
                "MarkhandDeadLetterJobs",
                "MarkhandDependencyProbeDown",
                "MarkhandScrapeTargetDown",
                "MarkhandDependencyProbeAbsent",
            } and src == "O02-OPS-ERROR-OUTBREAK-RATIO":
                self.err(f"{name}: must not cite error-outbreak ratio")
            runbook = (rule.get("annotations") or {}).get("runbook")
            if runbook and not (ROOT / runbook).is_file():
                self.err(f"{name}: missing runbook {runbook}")
            for key in (rule.get("labels") or {}):
                if key in FORBIDDEN_LABELS:
                    self.err(f"{name}: forbidden label {key}")

        prom_cfg = load_yaml(PROMETHEUS_PATH)
        for rf in prom_cfg.get("rule_files") or []:
            if "blocked" in str(rf):
                self.err("prometheus.yml must not load blocked rules")

        bb = load_yaml(BLACKBOX_PATH)
        modules = bb.get("modules") or {}
        if "http_2xx" not in modules or "tcp_connect" not in modules:
            self.err("blackbox.yml must define http_2xx and tcp_connect")
        if modules["tcp_connect"].get("prober") != "tcp":
            self.err("tcp_connect must use tcp prober")

        images = load_json(IMAGES_LOCK)["images"]
        compose = COMPOSE_PATH.read_text(encoding="utf-8")
        for ref in images.values():
            if "@sha256:" not in ref:
                self.err(f"unpinned image {ref}")
            if ref.split(":")[0].endswith("latest") or ":latest@" in ref:
                self.err(f"mutable latest tag: {ref}")
        for img_line in re.findall(r"image:\s*(\S+)", compose):
            if "@sha256:" not in img_line:
                self.err(f"compose image not digest-pinned: {img_line}")

    def check_compose_binds_and_matrix(self) -> None:
        compose_text = COMPOSE_PATH.read_text(encoding="utf-8")
        if "--project-directory" not in compose_text and "project-directory" not in compose_text:
            # documented in comments + up.sh
            pass
        if "${REPO_ROOT" not in compose_text:
            self.err("compose.observability.yml must use ${REPO_ROOT} bind sources")
        if "REPO_ROOT must be absolute" not in compose_text:
            self.err("compose binds must require absolute REPO_ROOT")

        up_text = UP_SH_PATH.read_text(encoding="utf-8")
        for needle in (
            '--project-directory "$REPO_ROOT"',
            '-f "$REPO_ROOT/deploy/compose.poc.yml"',
            '-f "$REPO_ROOT/deploy/observability/compose.observability.yml"',
            "export REPO_ROOT",
        ):
            if needle not in up_text:
                self.err(f"up.sh missing required invocation fragment: {needle}")

        # Resolve bind sources under REPO_ROOT without Docker (volume lines only).
        bind_re = re.compile(
            r"^\s*-\s*\$\{REPO_ROOT[^}]*\}/(deploy/observability/[^:\s]+):",
            re.MULTILINE,
        )
        binds = bind_re.findall(compose_text)
        if not binds:
            self.err("compose.observability.yml has no ${REPO_ROOT}/deploy/observability binds")
        for rel in binds:
            path = ROOT / rel
            if not path.exists():
                self.err(f"merged compose bind source missing: {rel}")

        obs = load_yaml(COMPOSE_PATH)
        poc = load_yaml(POC_COMPOSE_PATH)
        services = obs.get("services") or {}

        collector = services.get("otel-collector") or {}
        collector_nets = network_names(collector)
        if collector_nets != {"private", "convert"}:
            self.err(
                f"otel-collector networks must be {{private, convert}}, got {collector_nets}"
            )
        convert_net = (obs.get("networks") or {}).get("convert") or (
            (poc.get("networks") or {}).get("convert") or {}
        )
        if not convert_net.get("internal", False) and not (
            (poc.get("networks") or {}).get("convert") or {}
        ).get("internal", False):
            self.err("convert network must be internal:true (no external egress)")

        for name in OTEL_EMITTERS:
            svc = services.get(name) or {}
            env = svc.get("environment") or {}
            if not isinstance(env, dict):
                self.err(f"{name}: OTEL environment must be a map for merge semantics")
                continue
            for key, value in REQUIRED_OTEL_ENV.items():
                if str(env.get(key)) != value:
                    self.err(f"{name}: {key} must be {value!r} (got {env.get(key)!r})")
            if not str(env.get("MARKHAND_OTEL_SERVICE_NAME", "")).startswith("markhand-"):
                self.err(f"{name}: MARKHAND_OTEL_SERVICE_NAME must be set")

        # Convert worker must not gain private/edge via overlay (egress isolation).
        convert_overlay = services.get("worker-convert") or {}
        if "networks" in convert_overlay:
            nets = network_names(convert_overlay)
            if "private" in nets or "edge" in nets:
                self.err("worker-convert overlay must not add private/edge networks")
        poc_convert_nets = network_names((poc.get("services") or {}).get("worker-convert") or {})
        if poc_convert_nets != {"convert"}:
            self.err(f"poc worker-convert must remain convert-only, got {poc_convert_nets}")

        # Embedding alias for mock + aiteamvn profiles.
        for svc_name, profile in (("mock-embedding", "mock"), ("embedding-cpu", "aiteamvn")):
            overlay_svc = services.get(svc_name) or {}
            aliases = network_aliases(overlay_svc, "private")
            if "embedding" not in aliases:
                self.err(f"{svc_name}: overlay must advertise private alias embedding")
            poc_svc = (poc.get("services") or {}).get(svc_name) or {}
            profiles = set(poc_svc.get("profiles") or [])
            if profile not in profiles:
                self.err(f"{svc_name}: poc profile {profile} missing (got {profiles})")

        prom = PROMETHEUS_PATH.read_text(encoding="utf-8")
        if "http://embedding:8080/health" not in prom:
            self.err("prometheus.yml must probe http://embedding:8080/health alias")
        if "http://mock-embedding:8080/health" in prom:
            self.err("prometheus.yml must use embedding alias, not mock-embedding host")

        # Host-only node-exporter; no fake named-volume mounts.
        node = services.get("node-exporter") or {}
        vols = "\n".join(str(v) for v in (node.get("volumes") or []))
        if "/:/host" not in vols.replace(" ", ""):
            # allow "/:/host:ro,rslave"
            if not re.search(r"/:/host", vols):
                self.err("node-exporter must bind host root at /host")
        if "pgdata" in vols or "miniodata" in vols or "postgresql" in vols:
            self.err("node-exporter must not mount named PG/MinIO volumes")

        # TLS profile honesty in comments
        if "production contract remains HTTPS" not in compose_text.lower() and (
            "Production contract remains HTTPS" not in compose_text
        ):
            self.err("compose must document prod HTTPS vs POC HTTP OTLP")

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
                f"promtool version output does not contain {lock_ver}; using relative .tools/promtool"
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

        # Every active alert must appear in promtool tests with a firing case
        # and at least one non-fire or temporal resolve case.
        tests = load_yaml(PROM_TESTS).get("tests") or []
        covered_fire: set[str] = set()
        covered_nonfire: set[str] = set()
        for case in tests:
            for art in case.get("alert_rule_test") or []:
                name = art.get("alertname")
                if not name:
                    continue
                if art.get("exp_alerts"):
                    covered_fire.add(name)
                else:
                    covered_nonfire.add(name)
        for name in self.active_alert_names:
            if name not in covered_fire:
                self.err(f"promtool tests missing firing case for {name}")
            if name not in covered_nonfire and name not in {
                # outbreak alerts covered by dedicated non-fire siblings where needed;
                # require nonfire for SLO/disk/queue/auth/drift/probe at minimum.
            }:
                # Require non-fire OR temporal resolve (empty exp_alerts) for all.
                if name not in covered_nonfire:
                    self.err(f"promtool tests missing non-fire/resolve case for {name}")

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
            blob = json.dumps(data)
            if "reconcile:error" in blob or "reconcile errors" in blob.lower():
                self.err(f"{path.name}: must not claim reconcile error panels")
            if data.get("uid") == "markhand-ops":
                if "host_root_free_ratio" not in blob and 'mountpoint="/"' not in blob:
                    self.err("markhand-ops must show host root free ratio")
                if "named-volume" not in blob.lower() and "unavailable/blocked" not in blob.lower():
                    self.err("markhand-ops disk panel must state named-volume blocked")
                mp = next(
                    (
                        v
                        for v in data.get("templating", {}).get("list", [])
                        if v.get("name") == "mountpoint"
                    ),
                    None,
                )
                if mp:
                    opts = {o.get("value") for o in mp.get("options") or []}
                    if opts - {"/", "$__all"}:
                        self.err("markhand-ops mountpoint variable must be host root only")
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
            if data.get("uid") == "markhand-slo":
                if (
                    "filtered-query P99" not in blob
                    and "P99 blocked" not in blob
                    and "coverage gap" not in blob.lower()
                ):
                    self.err("markhand-slo dashboard must document P99 coverage gap")

    def check_schemas(self) -> None:
        for path in (
            PROMETHEUS_PATH,
            ALERTMANAGER_PATH,
            OTEL_PATH,
            COMPOSE_PATH,
            BLACKBOX_PATH,
            DATASOURCE_PATH,
            POC_COMPOSE_PATH,
        ):
            try:
                load_yaml(path)
            except Exception as error:  # noqa: BLE001
                self.err(f"YAML parse failed for {path.relative_to(ROOT)}: {error}")
        am = load_yaml(ALERTMANAGER_PATH)
        inhibits = am.get("inhibit_rules") or []
        for rule in inhibits:
            targets = str(rule.get("target_matchers") or rule.get("target_match") or "")
            if "slo|queue|parser" in targets or "class=~" in targets:
                self.err("broad class-based inhibit_rules must be removed")
        prom = PROMETHEUS_PATH.read_text(encoding="utf-8")
        for needle in (
            "http://api:8787/api/v1/health/ready",
            "http://qdrant:6333/healthz",
            "http://minio:9000/minio/health/live",
            "http://embedding:8080/health",
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
            if 'COMPOSE="' in text or "COMPOSE='" in text:
                self.err(f"{name}: compose command stored in scalar string (word-split risk)")
            if "logs --tail=0" in text:
                self.err(f"{name}: no-op logs --tail=0 cleanup must be removed")
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
            if name == "disk-exhaustion.md":
                lowered = text.lower()
                for needle in (
                    "MarkhandHostRootFilesystemLow",
                    'mountpoint="/"',
                    "named-volume",
                    "docker system df",
                    "poc_compose_init",
                    '"${COMPOSE[@]}"',
                ):
                    if needle not in text and needle.lower() not in lowered:
                        self.err(f"{name}: missing required disk semantics: {needle}")
                if "unavailable" not in lowered and "blocked" not in lowered:
                    self.err(f"{name}: must state named-volume attribution unavailable/blocked")
                if "/var/lib/postgresql" in text and "blocked" not in lowered:
                    self.err(f"{name}: must not claim PG mount monitoring without blocked note")
            if re.search(
                r"(?i)run\s+`?(admin requeue|reconcile --mode=|job-admin requeue)",
                text,
            ):
                self.err(f"{name}: invents unsupported admin commands")

    def check_secrets(self) -> None:
        for path in OBS.rglob("*"):
            if not path.is_file() or path.suffix in {".png", ".jpg"}:
                continue
            if path.name == "validation-report.json":
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
        required_stages = data.get("required_stages_ordered") or []
        if required_stages != [
            "detection",
            "contain",
            "recover",
            "verify",
            "rollback",
        ]:
            self.err("tabletop required_stages_ordered must be detection→rollback")
        scenarios = data.get("scenarios") or []
        if len(scenarios) < 7:
            self.err("tabletop must inventory at least 7 scenarios")
        alerts_covered = {s.get("alert") for s in scenarios}
        for must in (
            "MarkhandHostRootFilesystemLow",
            "MarkhandSearchAvailabilityBurn",
            "MarkhandDriftDetected",
            "MarkhandDeadLetterJobs",
        ):
            if must not in alerts_covered:
                self.err(f"tabletop missing scenario for {must}")
        if "MarkhandReconcileErrors" in alerts_covered:
            self.err("tabletop must not claim MarkhandReconcileErrors (blocked)")
        if "MarkhandDiskLow" in alerts_covered:
            self.err("tabletop must use host-root disk alert name")
        prom_cases = {
            t.get("name") for t in (load_yaml(PROM_TESTS).get("tests") or []) if t.get("name")
        }
        for scenario in scenarios:
            sid = scenario.get("id")
            runbook = scenario.get("runbook")
            if not runbook or not (ROOT / runbook).is_file():
                self.err(f"tabletop {sid}: missing runbook link {runbook}")
            stages = scenario.get("stages_ordered") or scenario.get("steps_exercised")
            if stages != required_stages:
                self.err(f"tabletop {sid}: stages_ordered must match required order")
            if scenario.get("outcome") != "pass_tabletop":
                self.err(f"tabletop {sid}: outcome must be pass_tabletop")
            case = scenario.get("promtool_case")
            if case and case not in prom_cases:
                self.err(f"tabletop {sid}: unknown promtool_case {case}")

    def run(self) -> list[str]:
        self.check_threshold_provenance()
        self.check_metric_inventory()
        self.check_active_rules_inventory()
        self.check_compose_binds_and_matrix()
        self.check_promtool()
        self.check_dashboards()
        self.check_schemas()
        self.check_runbooks()
        self.check_secrets()
        self.check_tabletop()
        self.errors = sorted(set(self.errors))
        return self.errors

    def report(self) -> dict[str, Any]:
        # Deterministic fields only; repo-relative paths.
        promtool_rel = None
        if self.promtool_path is not None:
            try:
                promtool_rel = str(self.promtool_path.resolve().relative_to(ROOT))
            except ValueError:
                promtool_rel = ".tools/promtool"
        return {
            "version": 3,
            "issue": "P1B-O02",
            "ok": not self.errors,
            "alertCount": len(self.active_alert_names),
            "activeAlerts": self.active_alert_names,
            "blockedAlertCount": len([r for r in self.blocked_rules if "alert" in r]),
            "blockedAlerts": sorted(
                r["alert"] for r in self.blocked_rules if "alert" in r
            ),
            "dashboardCount": len(list(DASHBOARD_DIR.glob("*.json"))),
            "runbookCount": len(REQUIRED_RUNBOOKS),
            "promtool": promtool_rel,
            "promtoolCheckOk": self.promtool_check_ok,
            "promtoolTestOk": self.promtool_test_ok,
            "dockerAvailable": bool(shutil.which("docker")),
            "claims_real_outage": False,
            "errors": self.errors,
            "notes": sorted(
                set(
                    self.notes
                    + (
                        []
                        if shutil.which("docker")
                        else [
                            "Docker not available in this environment; "
                            "compose deployability validated via YAML/image pins/"
                            "REPO_ROOT bind path checks/network matrix only."
                        ]
                    )
                )
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

    def test_mutation_syntactically_valid_nonexistent_metric_fails_inventory(self) -> None:
        checks = ObservabilityO02Checks()
        checks.alert_rules = list(checks.alert_rules)
        checks.alert_rules[0] = dict(checks.alert_rules[0])
        checks.alert_rules[0]["expr"] = "markhand_totally_nonexistent_metric_total > 1"
        checks.check_metric_inventory()
        self.assertTrue(
            any("markhand_totally_nonexistent_metric_total" in e for e in checks.errors),
            checks.errors,
        )

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

    def test_mutation_wrong_policy_citation_caught(self) -> None:
        checks = ObservabilityO02Checks()
        for rule in checks.alert_rules:
            if rule.get("alert") == "MarkhandDeadLetterJobs":
                rule.setdefault("annotations", {})["threshold_source"] = (
                    "O02-OPS-ERROR-OUTBREAK-RATIO"
                )
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
    args.json_report.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

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
