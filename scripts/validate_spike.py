#!/usr/bin/env python3
"""Validate P0-04 spike configuration and measured environment fingerprint."""

from __future__ import annotations

import argparse
import hashlib
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
RUNTIME_SERVICES = {"postgres", "qdrant", "minio", "otel", "mock-embedding"}
IMAGE_SERVICES = {*RUNTIME_SERVICES, "minio-init"}
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


def compose_command(
    nested_enabled: bool = False,
    gpu_enabled: bool = False,
) -> list[str]:
    values = env_values(ENV_FILE)
    command = [
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
    if nested_enabled:
        command.extend(["-f", str(ROOT / "deploy/spike/compose.nested.yml")])
    if gpu_enabled:
        command.extend(["--profile", "gpu"])
    return command


def validate_config() -> list[str]:
    errors = []
    override = (ROOT / "deploy/compose.spike.yml").read_text(encoding="utf-8")
    for service in (*RUNTIME_SERVICES, "vllm"):
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
    profiles = payload.get("profiles", [])
    gpu_enabled = "gpu" in profiles
    nested_enabled = "nested-network-workaround" in profiles
    expected_runtime = set(RUNTIME_SERVICES)
    expected_images = set(IMAGE_SERVICES)
    if gpu_enabled:
        expected_runtime.add("vllm")
        expected_images.add("vllm")
    fingerprint = payload.get("environment", {}).get("fingerprint", {})
    hardware = fingerprint.get("hardware", {})
    report_commit = str(payload.get("git", {}).get("commit", ""))
    if not re.fullmatch(r"[0-9a-f]{40}", report_commit):
        errors.append("spike report git commit is invalid")
    elif fingerprint.get("gitCommit") != report_commit:
        errors.append("spike report commit fields disagree")
    else:
        ancestor = subprocess.run(
            ["git", "merge-base", "--is-ancestor", report_commit, "HEAD"],
            cwd=ROOT,
            check=False,
        )
        if ancestor.returncode != 0:
            errors.append("spike report commit is not an ancestor of HEAD")
        else:
            changed = subprocess.check_output(
                ["git", "diff", "--name-only", report_commit, "HEAD"],
                cwd=ROOT,
                text=True,
            ).splitlines()
            allowed = {
                "bench/markhand_web/reports/spike-environment.json",
                "plans/markhand-web/backlog/phase-0/issues/README.md",
                "plans/markhand-web/roadmap.html",
            }
            if any(item not in allowed for item in changed):
                errors.append("spike report is stale relative to implementation")
    if payload.get("git", {}).get("dirty") is not False:
        errors.append("spike report must be captured from a clean tracked tree")
    for field in ("composeFileSha256", "fixtureManifestSha256"):
        if not SHA256.fullmatch(str(fingerprint.get(field, ""))):
            errors.append(f"spike report {field} is invalid")
    fixture_path = payload.get("fixtureManifestPath")
    if not isinstance(fixture_path, str):
        errors.append("spike report fixture manifest path is missing")
    else:
        resolved_fixture = (ROOT / fixture_path).resolve()
        if (
            not resolved_fixture.is_relative_to(ROOT.resolve())
            or not resolved_fixture.is_file()
            or hashlib.sha256(resolved_fixture.read_bytes()).hexdigest()
            != fingerprint.get("fixtureManifestSha256")
        ):
            errors.append("spike report fixture manifest fingerprint mismatch")
    if shutil.which("docker"):
        rendered = subprocess.check_output(
            [
                *compose_command(
                    nested_enabled=nested_enabled,
                    gpu_enabled=gpu_enabled,
                ),
                "config",
            ],
            cwd=ROOT,
        )
        if hashlib.sha256(rendered).hexdigest() != fingerprint.get(
            "composeFileSha256"
        ):
            errors.append("spike report compose fingerprint mismatch")
    if set(fingerprint.get("imageDigests", {})) != expected_images:
        errors.append("spike report image digests are incomplete")
    else:
        for service, encoded in fingerprint["imageDigests"].items():
            try:
                digests = json.loads(encoded)
            except (TypeError, json.JSONDecodeError):
                digests = []
            if not digests or not all(
                re.search(r"@sha256:[0-9a-f]{64}$", digest)
                for digest in digests
            ):
                errors.append(f"spike report image digest is invalid: {service}")
            elif shutil.which("docker"):
                image_ref = fingerprint.get("serviceVersions", {}).get(service)
                inspected = subprocess.run(
                    [
                        "docker",
                        "image",
                        "inspect",
                        "--format",
                        "{{json .RepoDigests}}",
                        str(image_ref),
                    ],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                if inspected.returncode != 0 or inspected.stdout.strip() != encoded:
                    errors.append(
                        f"spike report image digest differs from local image: {service}"
                    )
    if set(fingerprint.get("serviceVersions", {})) != expected_images:
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
        expected_runtime
    ) or result.get("pass") is not True:
        errors.append("spike report health result is invalid")
    if payload.get("targetMatch") is True and (
        nested_enabled
        or hardware.get("disk", {}).get("type") != "nvme"
        or hardware.get("disk", {}).get("iopsVerified") is not True
        or hardware.get("gpu", {}).get("count", 0) < 1
    ):
        errors.append("spike report falsely claims Profile B target match")
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
