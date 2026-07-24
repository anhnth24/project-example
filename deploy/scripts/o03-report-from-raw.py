#!/usr/bin/env python3
"""Reproducible O03 report generator from raw evidence only."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import pathlib
import re
import sys


def load_lines(path: pathlib.Path) -> list[str]:
    if not path.is_file():
        return []
    return [
        ln.strip() for ln in path.read_text(encoding="utf-8").splitlines() if ln.strip()
    ]


def read_int(path: pathlib.Path) -> int | None:
    if not path.is_file():
        return None
    text = path.read_text(encoding="utf-8").strip()
    if not text:
        return None
    return int(text)


def read_text(path: pathlib.Path, default: str = "") -> str:
    return path.read_text(encoding="utf-8").strip() if path.is_file() else default


def read_json(path: pathlib.Path) -> dict:
    if not path.is_file():
        return {}
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return {}
    return value if isinstance(value, dict) else {}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("raw_dir", type=pathlib.Path)
    ap.add_argument("--out-dir", type=pathlib.Path, required=True)
    args = ap.parse_args()
    raw = args.raw_dir.resolve()
    out = args.out_dir.resolve()
    if not raw.is_dir():
        print(f"raw dir missing: {raw}", file=sys.stderr)
        return 2

    stamp = raw.name.removeprefix("o03-") if raw.name.startswith("o03-") else raw.name
    passes = load_lines(raw / "passes.txt")
    gaps = load_lines(raw / "gaps.txt")
    capture_window = read_int(raw / "capture-window.seconds")
    restore_green_s = read_int(raw / "restore-green.seconds")
    # Legacy filenames still accepted for window fallbacks.
    if capture_window is None:
        capture_window = read_int(raw / "rpo.seconds")
    if restore_green_s is None:
        restore_green_s = read_int(raw / "rto.seconds")

    consistency_rpo = read_int(raw / "consistency-rpo.seconds")
    query_ready_rto = read_int(raw / "query-ready-rto.seconds")
    full_vector_rto = read_int(raw / "full-vector-rto.seconds")
    baseline = read_text(raw / "api-ready-baseline.status", "000")
    post_ready = read_text(raw / "api-ready-post-restore.status", "n/a")
    post_live = read_text(raw / "api-live-post-restore.status", "n/a")
    green_pre_ready = read_text(raw / "api-ready-green-pre-attest.status", "n/a")
    blue_during_green = read_text(raw / "api-ready-blue-during-green.status", "n/a")
    key_id = "redacted"
    if (raw / "backup-meta" / "manifest.json").is_file():
        try:
            man = json.loads((raw / "backup-meta" / "manifest.json").read_text())
            key_id = (man.get("trustedBoundary") or {}).get("keyId") or key_id
        except json.JSONDecodeError:
            pass

    cleanup_ok = False
    if (raw / "cleanup-verify.txt").is_file():
        cleanup_ok = "cleanup_verified=1" in (raw / "cleanup-verify.txt").read_text()

    attestation = read_json(raw / "green-target-attestation.json")
    required_checks = {
        "manifestAuthenticated",
        "postgresConsistent",
        "minioConsistent",
        "qdrantConsistent",
        "crossStoreRefsConsistent",
        "restoreFenceMatches",
    }
    attestation_checks = attestation.get("checks") or {}
    attestation_ok = (
        attestation.get("schemaVersion") == 1
        and attestation.get("kind") == "markhand.green-target-state"
        and all(attestation_checks.get(name) is True for name in required_checks)
    )
    query_proof = read_json(raw / "green-query-proof.json")
    query_proof_ok = (
        query_proof.get("loginHttp") == 200
        and query_proof.get("askHttp") == 200
        and query_proof.get("grounded") is True
        and query_proof.get("expectedDocument") is True
    )
    encryption = read_text(raw / "encryption-policy.txt")
    encryption_ok = (
        "MARKHAND_BACKUP_ENCRYPTED=1" in encryption
        and "marker_verified=1" in encryption
    )
    consistency_rpo_pass = consistency_rpo is not None and 0 <= consistency_rpo <= 900
    query_ready_rto_pass = query_ready_rto is not None and 0 <= query_ready_rto <= 3600
    full_vector_rto_pass = full_vector_rto is not None and 0 <= full_vector_rto <= 14400
    provenance = read_json(raw / "provenance.json")
    expected_services = {
        "api",
        "minio",
        "postgres",
        "qdrant",
        "worker-convert",
        "worker-index",
    }
    image_ids = (
        provenance.get("imageIds")
        if isinstance(provenance.get("imageIds"), dict)
        else {}
    )
    provenance_fields_ok = (
        isinstance(provenance.get("gitShaFull"), str)
        and re.fullmatch(r"[0-9a-f]{40}", provenance["gitShaFull"]) is not None
        and provenance.get("gitDirty") is False
        and isinstance(provenance.get("composeProject"), str)
        and bool(provenance["composeProject"])
        and all(
            isinstance(provenance.get(name), str)
            and re.fullmatch(r"[0-9a-f]{64}", provenance[name]) is not None
            for name in (
                "migrationManifestSha256",
                "composeFileSha256",
                "indexSignature",
            )
        )
        and expected_services.issubset(image_ids)
        and all(
            isinstance(image_ids.get(service), str) and bool(image_ids[service])
            for service in expected_services
        )
    )
    blockers: list[str] = []
    if gaps:
        blockers.append("raw drill recorded acceptance gaps")
    if baseline != "200":
        blockers.append("baseline API was not ready")
    if green_pre_ready != "503":
        blockers.append("green API did not fail closed before attestation")
    if post_live != "200" or post_ready != "200":
        blockers.append("attested green API was not live and ready")
    if blue_during_green != "503":
        blockers.append("blue API was not fenced during green validation")
    if not attestation_ok:
        blockers.append("independent green attestation missing or incomplete")
    if not query_proof_ok:
        blockers.append("restored green API query proof missing or incomplete")
    if not consistency_rpo_pass:
        blockers.append("consistency RPO evidence missing or above target")
    if not query_ready_rto_pass:
        blockers.append("query-ready RTO evidence missing or above target")
    if not full_vector_rto_pass:
        blockers.append("full-vector RTO evidence missing or above target")
    if provenance.get("gitDirty") is True:
        blockers.append("source git worktree was dirty")
    elif not provenance_fields_ok:
        blockers.append("release provenance missing or incomplete")
    if not encryption_ok:
        blockers.append("encrypted destination policy evidence missing")
    if not cleanup_ok:
        blockers.append("cleanup verification failed or missing")
    status = "pass" if not blockers else "in_progress"
    notes = [
        "Report generated solely from raw evidence (reproducible).",
        "Traffic cutover is not claimed; acceptance uses a distinct restored green API.",
        "The restore fence clears only after independent target-state attestation.",
    ]
    manifest_path = raw / "raw-manifest.json"
    raw_artifact_manifest = None
    if manifest_path.is_file():
        raw_artifact_manifest = {
            "path": str(manifest_path),
            "sha256": hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
        }

    payload = {
        "issue": "P1B-O03",
        "stamp": stamp,
        "status": status,
        "captureWindowSeconds": capture_window,
        "restoreGreenSeconds": restore_green_s,
        "consistencyRpoSeconds": consistency_rpo,
        "queryReadyRtoSeconds": query_ready_rto,
        "fullVectorRtoSeconds": full_vector_rto,
        "rpoSecondsMeasured": consistency_rpo,
        "queryReadyRtoSecondsMeasured": query_ready_rto,
        "fullVectorRtoSecondsMeasured": full_vector_rto,
        "consistencyRpoPass": consistency_rpo_pass,
        "queryReadyRtoPass": query_ready_rto_pass,
        "fullVectorRtoPass": full_vector_rto_pass,
        "rpoSecondsTarget": 900,
        "queryReadyRtoSecondsTarget": 3600,
        "fullVectorRtoSecondsTarget": 14400,
        "baselineReadyHttp": baseline,
        "greenPreAttestationReadyHttp": green_pre_ready,
        "postRestoreLiveHttp": post_live,
        "postRestoreReadyHttp": post_ready,
        "blueReadyDuringGreenHttp": blue_during_green,
        "attestationPassed": attestation_ok,
        "greenQueryPassed": query_proof_ok,
        "encryptedDestinationPolicyPassed": encryption_ok,
        "cleanupVerified": cleanup_ok,
        "passes": passes,
        "gaps": gaps,
        "blockers": blockers,
        "trustedBoundary": f"hmac_sha256 keyId={key_id} (key env-only, redacted)",
        "rawDir": str(raw),
        "provenance": provenance,
        "generatedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "notes": notes,
    }
    if raw_artifact_manifest is not None:
        payload["rawArtifactManifest"] = raw_artifact_manifest
    out.mkdir(parents=True, exist_ok=True)
    (out / "o03-restore.json").write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8"
    )
    md = [
        "# P1B-O03 backup/restore drill",
        "",
        f"- Status: `{payload['status']}`",
        f"- Capture window: `{payload['captureWindowSeconds']}`s",
        f"- Restore-green seconds: `{payload['restoreGreenSeconds']}`s",
        f"- Consistency RPO seconds: `{payload['consistencyRpoSeconds']}`",
        f"- Query-ready RTO seconds: `{payload['queryReadyRtoSeconds']}`",
        f"- Full-vector RTO seconds: `{payload['fullVectorRtoSeconds']}`",
        f"- consistencyRpoPass: `{payload['consistencyRpoPass']}`",
        f"- queryReadyRtoPass: `{payload['queryReadyRtoPass']}`",
        f"- fullVectorRtoPass: `{payload['fullVectorRtoPass']}`",
        f"- Baseline ready: `{payload['baselineReadyHttp']}`",
        f"- Green pre-attestation ready: `{payload['greenPreAttestationReadyHttp']}`",
        f"- Post-restore live: `{payload['postRestoreLiveHttp']}`",
        f"- Post-restore ready: `{payload['postRestoreReadyHttp']}`",
        f"- Blue ready during green validation: `{payload['blueReadyDuringGreenHttp']}`",
        f"- Independent attestation: `{payload['attestationPassed']}`",
        f"- Restored query proof: `{payload['greenQueryPassed']}`",
        f"- Encrypted destination policy: `{payload['encryptedDestinationPolicyPassed']}`",
        f"- Cleanup verified: `{payload['cleanupVerified']}`",
        f"- Raw: `{raw}`",
        "",
        "## Passes",
        "",
    ]
    md += [f"- {p}" for p in passes] or ["- (none)"]
    md += ["", "## Exact gaps", ""]
    md += [f"- {g}" for g in gaps] or ["- (none recorded)"]
    md += ["", "## Blockers", ""]
    md += [f"- {blocker}" for blocker in blockers] or ["- (none)"]
    md += ["", "## Notes", ""] + [f"- {n}" for n in notes] + [""]
    (out / "o03-restore.md").write_text("\n".join(md), encoding="utf-8")
    print(out / "o03-restore.json")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
