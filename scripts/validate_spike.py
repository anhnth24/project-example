#!/usr/bin/env python3
"""Validate P0-04 spike configuration and measured environment fingerprint."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ENV_FILE = ROOT / "deploy/spike/.env.example"
REPORT = ROOT / "bench/markhand_web/reports/spike-environment.json"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SERVICES = {"postgres", "qdrant", "minio", "otel", "mock-embedding"}
SECRET = re.compile(
    r"(?:postgres(?:ql)?://[^/\s:@]+:[^@\s/]+@|AKIA[0-9A-Z]{16}|"
    r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----)"
)


def env_values(path: Path) -> dict[str, str]:
    values = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            values[key] = value
    return values


def compose_command() -> list[str]:
    values = env_values(ENV_FILE)
    return [
        "docker",
        "compose",
        "--env-file",
        str(ENV_FILE),
        "--project-name",
        values.get("MARKHAND_COMPOSE_PROJECT", "markhand-spike"),
        "-f",
        str(ROOT / "deploy/dev/compose.yml"),
        "-f",
        str(ROOT / "deploy/compose.spike.yml"),
    ]


def validate_config() -> list[str]:
    errors = []
    override = (ROOT / "deploy/compose.spike.yml").read_text(encoding="utf-8")
    for service in (*SERVICES, "vllm"):
        if not re.search(rf"^  {re.escape(service)}:\s*$", override, re.MULTILINE):
            errors.append(f"spike compose missing service override: {service}")
    if override.count("ports: !override") != 6:
        errors.append("spike compose must replace all six published port lists")
    values = env_values(ENV_FILE)
    required = {
        "MARKHAND_COMPOSE_PROJECT",
        "MARKHAND_SPIKE_VOLUME_PREFIX",
        "MARKHAND_SPIKE_POSTGRES_PORT",
        "MARKHAND_SPIKE_QDRANT_HTTP_PORT",
        "MARKHAND_SPIKE_MINIO_API_PORT",
        "MARKHAND_SPIKE_MOCK_EMBEDDING_PORT",
    }
    if not required.issubset(values):
        errors.append("spike env example is incomplete")
    ports = [
        value
        for key, value in values.items()
        if key.endswith("_PORT") and value.isdigit()
    ]
    if len(ports) != len(set(ports)):
        errors.append("spike host ports must be unique")
    if shutil.which("docker"):
        completed = subprocess.run(
            [*compose_command(), "config"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
        if completed.returncode != 0:
            errors.append(f"docker compose config failed: {completed.stderr}")
    return errors


def validate_report(path: Path = REPORT) -> list[str]:
    if not path.is_file():
        return [f"spike report missing: {path}"]
    payload = json.loads(path.read_text(encoding="utf-8"))
    errors = []
    fingerprint = payload.get("environment", {}).get("fingerprint", {})
    hardware = fingerprint.get("hardware", {})
    if not re.fullmatch(r"[0-9a-f]{40}", str(payload.get("git", {}).get("commit", ""))):
        errors.append("spike report git commit is invalid")
    for field in ("composeFileSha256", "fixtureManifestSha256"):
        if not SHA256.fullmatch(str(fingerprint.get(field, ""))):
            errors.append(f"spike report {field} is invalid")
    if set(fingerprint.get("imageDigests", {})) != SERVICES:
        errors.append("spike report image digests are incomplete")
    if set(fingerprint.get("serviceVersions", {})) != SERVICES:
        errors.append("spike report service versions are incomplete")
    if (
        not isinstance(hardware.get("cpu", {}).get("threads"), int)
        or hardware["cpu"]["threads"] < 1
        or not isinstance(hardware.get("ramGb"), (int, float))
        or hardware["ramGb"] <= 0
        or not isinstance(hardware.get("gpu", {}).get("count"), int)
    ):
        errors.append("spike report hardware is incomplete")
    result = payload.get("result", {})
    if result.get("metric") != "healthy_spike_services" or result.get("value") != len(
        SERVICES
    ) or result.get("pass") is not True:
        errors.append("spike report health result is invalid")
    if SECRET.search(path.read_text(encoding="utf-8")):
        errors.append("spike report contains a secret")
    return errors


class SpikeValidatorTests(unittest.TestCase):
    def test_config_contract_is_valid(self) -> None:
        self.assertEqual(validate_config(), [])

    def test_report_rejects_missing_or_incomplete_data(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "report.json"
            self.assertTrue(validate_report(path))
            path.write_text(json.dumps({"git": {"commit": "bad"}}))
            errors = validate_report(path)
            self.assertTrue(any("git commit" in error for error in errors))
            self.assertTrue(any("image digests" in error for error in errors))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config-only", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(SpikeValidatorTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    errors = validate_config()
    if not args.config_only:
        errors.extend(validate_report())
    if errors:
        for error in errors:
            print(f"- {error}")
        return 1
    print("P0-04 spike validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
