#!/usr/bin/env python3
"""Validate P0-04 spike configuration and measured environment fingerprint."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REPORT = ROOT / "bench/markhand_web/reports/spike-environment.json"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
RUNTIME_SERVICES = {"postgres", "qdrant", "minio", "otel", "mock-embedding"}
IMAGE_SERVICES = {*RUNTIME_SERVICES, "minio-init"}
SECRET = re.compile(
    r"(?:postgres(?:ql)?://[^/\s:@]+:[^@\s/]+@|AKIA[0-9A-Z]{16}|"
    r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----)"
)
IMPLEMENTATION_FILES = (
    "deploy/dev/compose.yml",
    "deploy/dev/otel-collector.yaml",
    "deploy/scripts/mock-embedding.py",
    "deploy/compose.spike.yml",
    "deploy/spike/common.sh",
    "deploy/spike/up.sh",
    "deploy/spike/health.sh",
    "deploy/spike/seed.sh",
    "deploy/spike/down.sh",
    "deploy/spike/reset.sh",
    "deploy/spike/verify-lifecycle.sh",
    "deploy/spike/images.lock.json",
    "scripts/validate_spike.py",
    "bench/markhand_web/scripts/fingerprint_spike.py",
)


def implementation_sha256() -> str:
    digest = hashlib.sha256()
    for relative in IMPLEMENTATION_FILES:
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update((ROOT / relative).read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def numeric(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
    )


def schema_errors(value: object, schema: dict, path: str) -> list[str]:
    errors: list[str] = []
    expected = schema.get("type")
    allowed = expected if isinstance(expected, list) else [expected] if expected else []
    matches = {
        "object": lambda item: isinstance(item, dict),
        "array": lambda item: isinstance(item, list),
        "string": lambda item: isinstance(item, str),
        "number": numeric,
        "integer": lambda item: isinstance(item, int) and not isinstance(item, bool),
        "boolean": lambda item: isinstance(item, bool),
        "null": lambda item: item is None,
    }
    if allowed and not any(matches[kind](value) for kind in allowed):
        return [f"{path}: schema type mismatch"]
    if "const" in schema and value != schema["const"]:
        errors.append(f"{path}: schema const mismatch")
    if isinstance(value, str):
        if len(value) < schema.get("minLength", 0):
            errors.append(f"{path}: string is too short")
        if schema.get("pattern") and not re.fullmatch(schema["pattern"], value):
            errors.append(f"{path}: string pattern mismatch")
    if numeric(value):
        if "minimum" in schema and value < schema["minimum"]:
            errors.append(f"{path}: number is below minimum")
        if "exclusiveMinimum" in schema and value <= schema["exclusiveMinimum"]:
            errors.append(f"{path}: number is below exclusive minimum")
    if isinstance(value, dict):
        if len(value) < schema.get("minProperties", 0):
            errors.append(f"{path}: object has too few properties")
        for field in schema.get("required", []):
            if field not in value:
                errors.append(f"{path}: missing required field {field}")
        for field, child in schema.get("properties", {}).items():
            if field in value:
                errors.extend(schema_errors(value[field], child, f"{path}.{field}"))
    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            errors.append(f"{path}: array has too few items")
        if isinstance(schema.get("items"), dict):
            for index, item in enumerate(value):
                errors.extend(schema_errors(item, schema["items"], f"{path}[{index}]"))
    return errors


def env_values(path: Path) -> dict[str, str]:
    values = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            values[key] = value
    return values


def default_env_file() -> Path:
    configured = os.environ.get("MARKHAND_SPIKE_ENV_FILE")
    if configured:
        return Path(configured).resolve()
    local = ROOT / "deploy/spike/.env"
    return local if local.is_file() else ROOT / "deploy/spike/.env.example"


def compose_command(
    env_file: Path | None = None,
    nested_enabled: bool = False,
    mock_enabled: bool = False,
    gpu_enabled: bool = False,
) -> list[str]:
    env_file = env_file or default_env_file()
    values = env_values(env_file)
    command = [
        "docker",
        "compose",
        "--env-file",
        str(env_file),
        "--project-name",
        values.get("MARKHAND_COMPOSE_PROJECT", "markhand-spike"),
        "-f",
        str(ROOT / "deploy/dev/compose.yml"),
        "-f",
        str(ROOT / "deploy/compose.spike.yml"),
    ]
    if nested_enabled:
        command.extend(["-f", str(ROOT / "deploy/spike/compose.nested.yml")])
    if mock_enabled:
        command.extend(["--profile", "mock"])
    if gpu_enabled:
        command.extend(["--profile", "gpu"])
    return command


def expected_target_match(hardware: dict, profiles: list[str]) -> bool:
    target = json.loads(
        (
            ROOT
            / "bench/markhand_web/environments/on-prem-reference.yaml"
        ).read_text()
    )
    return (
        "gpu" in profiles
        and "nested-network-workaround" not in profiles
        and hardware.get("cpu", {}).get("physicalCoresMeasured") is True
        and hardware.get("cpu", {}).get("cores", 0) >= target["cpu"]["cores"]
        and hardware.get("cpu", {}).get("threads", 0) >= target["cpu"]["threads"]
        and hardware.get("ramGb", 0) >= target["ramGb"]
        and hardware.get("disk", {}).get("type") == target["disk"]["type"]
        and hardware.get("disk", {}).get("capacityGb", 0)
        >= target["disk"]["capacityGb"]
        and hardware.get("disk", {}).get("iopsVerified") is True
        and hardware.get("disk", {}).get("iopsMeasured", 0) >= 100_000
        and SHA256.fullmatch(
            str(hardware.get("disk", {}).get("iopsEvidenceSha256", ""))
        )
        is not None
        and isinstance(hardware.get("disk", {}).get("iopsEvidence"), dict)
        and hardware["disk"]["iopsEvidence"].get("storageIdentitySha256")
        == hardware["disk"]["storageIdentitySha256"]
        and hardware["disk"]["iopsEvidence"].get("randomReadIops", 0)
        == hardware["disk"]["iopsMeasured"]
        and hardware.get("gpu", {}).get("count", 0) >= target["gpu"]["count"]
        and hardware.get("gpu", {}).get("vramGb", 0) >= target["gpu"]["vramGb"]
        and hardware.get("network", {}).get("bandwidthGbps", 0)
        >= target["network"]["bandwidthGbps"]
        and hardware.get("network", {}).get("bandwidthMeasured") is True
        and hardware.get("os", {}).get("distro") == target["os"]["distro"]
        and hardware.get("os", {}).get("arch") == target["os"]["arch"]
    )


def validate_config(env_file: Path | None = None) -> list[str]:
    env_file = env_file or default_env_file()
    errors = []
    override = (ROOT / "deploy/compose.spike.yml").read_text(encoding="utf-8")
    image_lock_payload = json.loads(
        (ROOT / "deploy/spike/images.lock.json").read_text(encoding="utf-8")
    )
    image_lock = image_lock_payload.get("images", {})
    if image_lock_payload.get("platform") != "linux/amd64":
        errors.append("spike image lock platform must be linux/amd64")
    for service in (*RUNTIME_SERVICES, "vllm"):
        if not re.search(rf"^  {re.escape(service)}:\s*$", override, re.MULTILINE):
            errors.append(f"spike compose missing service override: {service}")
    if override.count("ports: !override") != 6:
        errors.append("spike compose must replace all six published port lists")
    if set(image_lock) != {*IMAGE_SERVICES, "vllm"}:
        errors.append("spike image lock is incomplete")
    for service, image in image_lock.items():
        if not re.fullmatch(r".+@sha256:[0-9a-f]{64}", str(image)):
            errors.append(f"spike image lock digest is invalid: {service}")
        if f"image: {image}" not in override:
            errors.append(f"spike compose does not consume image lock: {service}")
    values = env_values(env_file)
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
        for profile_name, mock_enabled, gpu_enabled in (
            ("cpu-mock", True, False),
            ("gpu", False, True),
        ):
            completed = subprocess.run(
                [
                    *compose_command(
                        env_file,
                        mock_enabled=mock_enabled,
                        gpu_enabled=gpu_enabled,
                    ),
                    "config",
                ],
                cwd=ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            if completed.returncode != 0:
                errors.append(
                    f"docker compose config failed ({profile_name}): {completed.stderr}"
                )
                continue
            rendered_images = subprocess.check_output(
                [
                    *compose_command(
                        env_file,
                        mock_enabled=mock_enabled,
                        gpu_enabled=gpu_enabled,
                    ),
                    "config",
                    "--images",
                ],
                cwd=ROOT,
                text=True,
            ).splitlines()
            optional = "vllm" if mock_enabled else "mock-embedding"
            expected_images = {
                image for service, image in image_lock.items() if service != optional
            }
            if set(rendered_images) != expected_images:
                errors.append(
                    f"rendered spike images differ from image lock ({profile_name})"
                )
    return errors


def validate_report(
    path: Path = REPORT,
    env_file: Path | None = None,
) -> list[str]:
    env_file = env_file or default_env_file()
    if not path.is_file():
        return [f"spike report missing: {path}"]
    payload = json.loads(path.read_text(encoding="utf-8"))
    schema = json.loads(
        (
            ROOT
            / "bench/markhand_web/reports/environment-report.schema.json"
        ).read_text(encoding="utf-8")
    )
    errors = schema_errors(payload, schema, "report")
    profiles = payload.get("profiles", [])
    gpu_enabled = "gpu" in profiles
    nested_enabled = "nested-network-workaround" in profiles
    expected_runtime = set(RUNTIME_SERVICES)
    expected_images = set(IMAGE_SERVICES)
    if gpu_enabled:
        expected_runtime.add("vllm")
        expected_images.add("vllm")
    image_lock = json.loads(
        (ROOT / "deploy/spike/images.lock.json").read_text(encoding="utf-8")
    )["images"]
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
    if payload.get("implementationSha256") != implementation_sha256():
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
    if payload.get("fixtures", {}).get("manifestSha256") != fingerprint.get(
        "fixtureManifestSha256"
    ):
        errors.append("spike report fixture hash fields disagree")
    if payload.get("workloadProfileId") != fingerprint.get("workloadProfileId"):
        errors.append("spike report workload profile fields disagree")
    # Skip re-hashing rendered compose config: host-path sensitive; sources covered by implementationSha256.
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
                if (
                    inspected.returncode == 0
                    and inspected.stdout.strip() != encoded
                ):
                    errors.append(
                        f"spike report image digest differs from local image: {service}"
                    )
    if set(fingerprint.get("serviceVersions", {})) != expected_images:
        errors.append("spike report service versions are incomplete")
    else:
        for service in expected_images:
            if fingerprint["serviceVersions"].get(service) != image_lock.get(service):
                errors.append(
                    f"spike report service version differs from image lock: {service}"
                )
            expected_digest = str(image_lock.get(service, "")).rsplit("@", 1)[-1]
            try:
                reported_digests = json.loads(
                    fingerprint.get("imageDigests", {}).get(service, "[]")
                )
            except json.JSONDecodeError:
                reported_digests = []
            if not any(
                digest.endswith(f"@{expected_digest}")
                for digest in reported_digests
            ):
                errors.append(
                    f"spike report digest differs from image lock: {service}"
                )
    if (
        not isinstance(hardware.get("cpu", {}).get("vendor"), str)
        or not hardware["cpu"]["vendor"]
        or not isinstance(hardware.get("cpu", {}).get("model"), str)
        or not hardware["cpu"]["model"]
        or not isinstance(hardware.get("cpu", {}).get("cores"), int)
        or hardware["cpu"]["cores"] < 1
        or not isinstance(hardware.get("cpu", {}).get("threads"), int)
        or hardware["cpu"]["threads"] < 1
        or not isinstance(
            hardware.get("cpu", {}).get("physicalCoresMeasured"), bool
        )
        or not isinstance(hardware.get("ramGb"), (int, float))
        or hardware["ramGb"] <= 0
        or not isinstance(hardware.get("disk", {}).get("type"), str)
        or not hardware["disk"]["type"]
        or not isinstance(
            hardware.get("disk", {}).get("capacityGb"), (int, float)
        )
        or hardware["disk"]["capacityGb"] <= 0
        or not isinstance(hardware.get("gpu", {}).get("count"), int)
        or hardware["gpu"]["count"] < 0
        or not isinstance(hardware.get("gpu", {}).get("model"), str)
        or not isinstance(hardware.get("gpu", {}).get("vramGb"), (int, float))
        or hardware["gpu"]["vramGb"] < 0
        or not isinstance(
            hardware.get("network", {}).get("bandwidthGbps"), (int, float)
        )
        or hardware["network"]["bandwidthGbps"] < 0
        or not isinstance(
            hardware.get("network", {}).get("bandwidthMeasured"), bool
        )
        or not isinstance(hardware.get("network", {}).get("interface"), str)
        or not hardware["network"]["interface"]
        or not isinstance(
            hardware.get("network", {}).get("latencyMsAssumed"), (int, float)
        )
        or hardware["network"]["latencyMsAssumed"] < 0
        or not isinstance(hardware.get("os", {}).get("distro"), str)
        or not hardware["os"]["distro"]
        or not isinstance(hardware.get("os", {}).get("arch"), str)
        or not hardware["os"]["arch"]
        or not SHA256.fullmatch(
            str(hardware.get("disk", {}).get("storagePathSha256", ""))
        )
        or not isinstance(hardware.get("disk", {}).get("iopsMeasured"), int)
        or not isinstance(hardware.get("disk", {}).get("iopsVerified"), bool)
        or not isinstance(hardware.get("disk", {}).get("backingSource"), str)
    ):
        errors.append("spike report hardware is incomplete")
    disk = hardware.get("disk", {})
    if disk.get("iopsVerified"):
        evidence = disk.get("iopsEvidence")
        try:
            measured_at = dt.datetime.fromisoformat(
                str(evidence.get("measuredAt", "")).replace("Z", "+00:00")
            )
            generated_at = dt.datetime.fromisoformat(
                str(payload.get("generatedAt", "")).replace("Z", "+00:00")
            )
            evidence_age = generated_at - measured_at
        except (AttributeError, ValueError):
            evidence_age = dt.timedelta(days=999)
        evidence_hash = (
            hashlib.sha256(
                json.dumps(
                    evidence,
                    sort_keys=True,
                    separators=(",", ":"),
                ).encode()
            ).hexdigest()
            if isinstance(evidence, dict)
            else None
        )
        if (
            not isinstance(evidence, dict)
            or evidence.get("storageIdentitySha256")
            != disk.get("storageIdentitySha256")
            or evidence.get("randomReadIops") != disk.get("iopsMeasured")
            or evidence.get("readOnly") is not True
            or evidence.get("blockSizeBytes") != 4096
            or not isinstance(evidence.get("durationSeconds"), int)
            or evidence["durationSeconds"] < 30
            or not isinstance(evidence.get("tool"), str)
            or not evidence["tool"].strip()
            or not dt.timedelta(0)
            <= evidence_age
            <= dt.timedelta(hours=24)
            or evidence_hash != disk.get("iopsEvidenceSha256")
            or not SHA256.fullmatch(str(disk.get("iopsEvidenceSha256", "")))
        ):
            errors.append("spike report IOPS evidence is not storage-bound")
    elif (
        disk.get("iopsMeasured") != 0
        or disk.get("iopsEvidence") is not None
        or disk.get("iopsEvidenceSha256") is not None
    ):
        errors.append("spike report has unverified IOPS evidence")
    result = payload.get("result", {})
    if result.get("metric") != "healthy_spike_services" or result.get("value") != len(
        expected_runtime
    ) or result.get("pass") is not True:
        errors.append("spike report health result is invalid")
    lifecycle = payload.get("lifecycle", {})
    if (
        lifecycle.get("restartPersistence") is not True
        or lifecycle.get("resetDeletion") is not True
        or set(lifecycle.get("stores", []))
        != {"postgres", "qdrant", "minio"}
        or not isinstance(lifecycle.get("verifiedAt"), str)
        or not lifecycle["verifiedAt"].endswith("Z")
    ):
        errors.append("spike report lifecycle evidence is incomplete")
    expected_match = expected_target_match(hardware, profiles)
    if not isinstance(payload.get("targetMatch"), bool) or payload.get(
        "targetMatch"
    ) != expected_match:
        errors.append("spike report targetMatch does not match Profile B evidence")
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

    def test_report_rejects_missing_or_false_target_claim(self) -> None:
        if not REPORT.is_file():
            self.skipTest("measured spike report not generated")
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "report.json"
            payload = json.loads(REPORT.read_text())
            payload.pop("targetMatch", None)
            path.write_text(json.dumps(payload))
            self.assertTrue(
                any(
                    "targetMatch" in error
                    for error in validate_report(path)
                )
            )
            payload["targetMatch"] = True
            path.write_text(json.dumps(payload))
            self.assertTrue(
                any(
                    "targetMatch" in error
                    for error in validate_report(path)
                )
            )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config-only", action="store_true")
    parser.add_argument("--env-file", type=Path)
    parser.add_argument(
        "--report",
        type=Path,
        default=Path(
            os.environ.get("MARKHAND_SPIKE_REPORT", str(REPORT))
        ),
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(SpikeValidatorTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    env_file = args.env_file.resolve() if args.env_file else default_env_file()
    errors = validate_config(env_file)
    if not args.config_only:
        errors.extend(validate_report(args.report.resolve(), env_file))
    if errors:
        for error in errors:
            print(f"- {error}")
        return 1
    print("P0-04 spike validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
