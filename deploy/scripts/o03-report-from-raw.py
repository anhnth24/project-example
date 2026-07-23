#!/usr/bin/env python3
"""Reproducible O03 report generator from raw evidence only."""

from __future__ import annotations

import argparse
import datetime
import json
import pathlib
import sys


def load_lines(path: pathlib.Path) -> list[str]:
    if not path.is_file():
        return []
    return [ln.strip() for ln in path.read_text(encoding="utf-8").splitlines() if ln.strip()]


def read_int(path: pathlib.Path) -> int | None:
    if not path.is_file():
        return None
    text = path.read_text(encoding="utf-8").strip()
    if not text:
        return None
    return int(text)


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

    baseline = (
        (raw / "api-ready-baseline.status").read_text().strip()
        if (raw / "api-ready-baseline.status").is_file()
        else "000"
    )
    post_ready = (
        (raw / "api-ready-post-restore.status").read_text().strip()
        if (raw / "api-ready-post-restore.status").is_file()
        else "n/a"
    )
    post_live = (
        (raw / "api-live-post-restore.status").read_text().strip()
        if (raw / "api-live-post-restore.status").is_file()
        else "n/a"
    )
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

    status = "in_progress"
    notes = [
        "Report generated solely from raw evidence (reproducible).",
        "Promote/cutover disabled until API consumes routing + durable attestation.",
        "consistencyRpoPass/queryReadyRtoPass are null (not claimed).",
        "Status remains in_progress until all Sol acceptance items close.",
    ]
    if post_ready != "200":
        notes.append(
            f"No query-ready claim: post-restore ready HTTP={post_ready} (not 200)."
        )
    if not cleanup_ok:
        notes.append("Cleanup verification failed or missing — report should not be trusted.")

    payload = {
        "issue": "P1B-O03",
        "stamp": stamp,
        "status": status,
        "captureWindowSeconds": capture_window,
        "restoreGreenSeconds": restore_green_s,
        "consistencyRpoPass": None,
        "queryReadyRtoPass": None,
        "rpoSecondsTarget": 900,
        "queryReadyRtoSecondsTarget": 3600,
        "baselineReadyHttp": baseline,
        "postRestoreLiveHttp": post_live,
        "postRestoreReadyHttp": post_ready,
        "cleanupVerified": cleanup_ok,
        "passes": passes,
        "gaps": gaps,
        "trustedBoundary": f"hmac_sha256 keyId={key_id} (key env-only, redacted)",
        "rawDir": str(raw),
        "generatedAt": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "notes": notes,
    }
    out.mkdir(parents=True, exist_ok=True)
    (out / "o03-restore.json").write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    md = [
        "# P1B-O03 backup/restore drill",
        "",
        f"- Status: `{payload['status']}`",
        f"- Capture window: `{payload['captureWindowSeconds']}`s",
        f"- Restore-green seconds: `{payload['restoreGreenSeconds']}`s",
        f"- consistencyRpoPass: `{payload['consistencyRpoPass']}`",
        f"- queryReadyRtoPass: `{payload['queryReadyRtoPass']}`",
        f"- Baseline ready: `{payload['baselineReadyHttp']}`",
        f"- Post-restore live: `{payload['postRestoreLiveHttp']}`",
        f"- Post-restore ready: `{payload['postRestoreReadyHttp']}`",
        f"- Cleanup verified: `{payload['cleanupVerified']}`",
        f"- Raw: `{raw}`",
        "",
        "## Passes",
        "",
    ]
    md += [f"- {p}" for p in passes] or ["- (none)"]
    md += ["", "## Exact gaps", ""]
    md += [f"- {g}" for g in gaps] or ["- (none recorded)"]
    md += ["", "## Notes", ""] + [f"- {n}" for n in notes] + [""]
    (out / "o03-restore.md").write_text("\n".join(md), encoding="utf-8")
    print(out / "o03-restore.json")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
