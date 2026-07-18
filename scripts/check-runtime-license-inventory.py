#!/usr/bin/env python3
"""Measure the model/native runtime redistribution-license gate."""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_INVENTORY = ROOT / "docs/markhand-web-runtime-license-inventory.json"
DEFAULT_REQUIREMENTS = ROOT / "docs/markhand-web-runtime-license-requirements.json"
KINDS = {"model", "native-library", "container-image", "dataset"}
DISPOSITIONS = {"approved", "excluded"}
REDISTRIBUTION = {"allowed", "forbidden", "source-offer-required"}
SHA256 = re.compile(r"^[0-9a-f]{64}$")
UNKNOWN_LICENSES = {"unknown", "tbd", "unlicensed", "proprietary-unknown"}


def load(path: Path, label: str) -> tuple[dict | None, list[str]]:
    if not path.is_file():
        return None, [f"{label} missing: {path}"]
    try:
        return json.loads(path.read_text(encoding="utf-8")), []
    except (OSError, json.JSONDecodeError) as error:
        return None, [f"{label} unreadable: {error}"]


def evidence_path(repository_root: Path, raw: object) -> Path | None:
    if not isinstance(raw, str) or not raw:
        return None
    pure = PurePosixPath(raw)
    if pure.is_absolute() or ".." in pure.parts or raw != pure.as_posix():
        return None
    path = (repository_root / pure).resolve()
    return path if path.is_relative_to(repository_root.resolve()) else None


def validate(
    inventory_path: Path,
    requirements_path: Path = DEFAULT_REQUIREMENTS,
    repository_root: Path = ROOT,
) -> tuple[list[str], float]:
    inventory, errors = load(inventory_path, "runtime license inventory")
    requirements, requirement_errors = load(
        requirements_path, "runtime license requirements"
    )
    errors.extend(requirement_errors)
    if inventory is None or requirements is None:
        return errors, 0.0

    entries = inventory.get("entries")
    required = requirements.get("required")
    if inventory.get("version") != 1 or not isinstance(entries, list) or not entries:
        errors.append("inventory requires version=1 and non-empty entries")
        return errors, 0.0
    if requirements.get("version") != 1 or not isinstance(required, list) or not required:
        errors.append("requirements require version=1 and non-empty required array")
        return errors, 0.0

    expected: dict[str, dict] = {}
    for item in required:
        item_id = item.get("id")
        kind = item.get("kind")
        allowed_licenses = item.get("allowedLicenses")
        checksum = item.get("artifactSha256")
        if (
            not isinstance(item_id, str)
            or not item_id
            or kind not in KINDS
            or not isinstance(allowed_licenses, list)
            or not allowed_licenses
            or any(not isinstance(name, str) or not name.strip() for name in allowed_licenses)
            or not isinstance(checksum, str)
            or not SHA256.fullmatch(checksum)
            or set(checksum) == {"0"}
        ):
            errors.append("requirements contain invalid id, kind, licenses, or checksum")
            continue
        if item_id in expected:
            errors.append(f"duplicate requirement id: {item_id}")
        expected[item_id] = item

    seen: set[str] = set()
    bundled = 0
    approved = 0
    for entry in entries:
        entry_id = entry.get("id")
        if not isinstance(entry_id, str) or not entry_id.strip():
            errors.append("inventory entry id must be non-empty")
            continue
        if entry_id in seen:
            errors.append(f"duplicate inventory id: {entry_id}")
        seen.add(entry_id)
        if entry_id not in expected:
            errors.append(f"unexpected inventory id: {entry_id}")
        requirement = expected.get(entry_id)
        if entry.get("kind") not in KINDS or (
            requirement and entry.get("kind") != requirement["kind"]
        ):
            errors.append(f"{entry_id}: invalid kind")
        for field in ("version", "source", "license"):
            if not isinstance(entry.get(field), str) or not entry[field].strip():
                errors.append(f"{entry_id}: missing {field}")
        license_name = str(entry.get("license", "")).strip().lower()
        if license_name in UNKNOWN_LICENSES:
            errors.append(f"{entry_id}: unresolved license")
        if requirement and entry.get("license") not in requirement["allowedLicenses"]:
            errors.append(f"{entry_id}: license is not allowed by requirements")
        checksum = entry.get("artifactSha256")
        if (
            not isinstance(checksum, str)
            or not SHA256.fullmatch(checksum)
            or set(checksum) == {"0"}
        ):
            errors.append(f"{entry_id}: invalid artifactSha256")
        elif requirement and checksum != requirement["artifactSha256"]:
            errors.append(f"{entry_id}: artifact checksum does not match requirements")
        disposition = entry.get("disposition")
        if disposition not in DISPOSITIONS:
            errors.append(f"{entry_id}: invalid disposition")
        redistribution = entry.get("redistribution")
        if redistribution not in REDISTRIBUTION:
            errors.append(f"{entry_id}: invalid redistribution")
        evidence = evidence_path(repository_root, entry.get("evidence"))
        if evidence is None or not evidence.is_file():
            errors.append(f"{entry_id}: evidence must be an existing repository file")
        is_bundled = entry.get("bundled")
        if not isinstance(is_bundled, bool):
            errors.append(f"{entry_id}: bundled must be boolean")
            continue
        if is_bundled:
            bundled += 1
            if disposition == "approved" and redistribution in {
                "allowed",
                "source-offer-required",
            }:
                approved += 1
            else:
                errors.append(f"{entry_id}: bundled runtime is not approved")

    missing = set(expected) - seen
    if missing:
        errors.append(f"inventory missing required entries: {sorted(missing)}")
    ratio = approved / bundled if bundled else 0.0
    if bundled == 0:
        errors.append("inventory has no bundled runtime entries")
    return errors, ratio


class LicenseInventoryTests(unittest.TestCase):
    def test_rejects_incomplete_unknown_or_unapproved_inventory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inventory = root / "inventory.json"
            requirements = root / "requirements.json"
            requirements.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "required": [
                            {
                                "id": "runtime",
                                "kind": "model",
                                "allowedLicenses": ["Apache-2.0"],
                                "artifactSha256": "a" * 64,
                            },
                            {
                                "id": "native",
                                "kind": "native-library",
                                "allowedLicenses": ["MIT"],
                                "artifactSha256": "b" * 64,
                            },
                        ],
                    }
                )
            )
            inventory.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "entries": [
                            {
                                "id": "runtime",
                                "kind": "model",
                                "version": "v1",
                                "source": "synthetic",
                                "license": "Mystery-License",
                                "artifactSha256": "bad",
                                "redistribution": "forbidden",
                                "disposition": "excluded",
                                "bundled": True,
                                "evidence": "missing.md",
                            }
                        ],
                    }
                )
            )
            errors, ratio = validate(inventory, requirements, root)
            self.assertEqual(ratio, 0.0)
            self.assertTrue(any("license is not allowed" in error for error in errors))
            self.assertTrue(any("invalid artifactSha256" in error for error in errors))
            self.assertTrue(any("not approved" in error for error in errors))
            self.assertTrue(any("missing required entries" in error for error in errors))

    def test_accepts_complete_approved_inventory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "evidence.md").write_text("synthetic evidence")
            inventory = root / "inventory.json"
            requirements = root / "requirements.json"
            requirements.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "required": [
                            {
                                "id": "runtime",
                                "kind": "model",
                                "allowedLicenses": ["Apache-2.0"],
                                "artifactSha256": "a" * 64,
                            }
                        ],
                    }
                )
            )
            inventory.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "entries": [
                            {
                                "id": "runtime",
                                "kind": "model",
                                "version": "v1",
                                "source": "synthetic",
                                "license": "Apache-2.0",
                                "artifactSha256": "a" * 64,
                                "redistribution": "allowed",
                                "disposition": "approved",
                                "bundled": True,
                                "evidence": "evidence.md",
                            }
                        ],
                    }
                )
            )
            errors, ratio = validate(inventory, requirements, root)
            self.assertEqual(errors, [])
            self.assertEqual(ratio, 1.0)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--inventory", type=Path, default=DEFAULT_INVENTORY)
    parser.add_argument("--requirements", type=Path, default=DEFAULT_REQUIREMENTS)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(LicenseInventoryTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    errors, ratio = validate(args.inventory, args.requirements)
    if errors:
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(json.dumps({"metric": "approved_runtime_licenses", "value": ratio}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
