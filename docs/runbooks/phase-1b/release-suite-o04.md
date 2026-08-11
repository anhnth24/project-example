# P1B-O04 — Vertical-slice / security release suite

## Purpose

Machine-verifiable release evidence for the single-org POC vertical slice:

upload → convert/index → citation (all `phase1b-mixed.yaml` ingest formats), plus
unauthorized/cross-tenant denial, suspend/membership/delete deny, adversarial
upload reject/contain, and worker hard-kill/replay consistency.

## Architecture (honest)

O04 pass requires the deployed POC Compose API/workers. The harness performs
black-box probes against the host-published API HTTP endpoint and records
`architecture.apiHttpExercised=true`. Cargo integration suites still cover
vertical/security internals, but an in-process-only run is explicitly non-pass.

Live pass also requires the F02 machine report selected by
`MARKHAND_F02_BOOT_REPORT` (default
`.artifacts/markhand_web/reports/poc-f02-boot.json`) with `passed=true` and matching
`composeProject`, `containerIds`, and `imageIds` for the POC Compose project.
O04 records sanitized API/test endpoints in provenance. The report also records
`composeFileSha256`, effective Compose config hash, F02 report/manifest hashes,
and a complete service map with Compose labels, container ids, image ids, health,
and port mappings.

## Evidence paths (O04 only)

| Artifact | Path |
|---|---|
| Report JSON | `bench/markhand_web/reports/phase-1b-gate/o04-release.json` |
| Report MD | `bench/markhand_web/reports/phase-1b-gate/o04-release.md` |
| Raw logs | `bench/markhand_web/reports/phase-1b-gate/raw/o04-<full-git-sha>/` |
| Raw manifest | `bench/markhand_web/reports/phase-1b-gate/raw/o04-<full-git-sha>/raw-manifest.json` |

Do **not** use or overwrite O05 `summary.json`.
Canonical `rawDir` is repo-relative and must stay under the O04 raw evidence root.
Every raw artifact listed in `raw-manifest.json` has `sha256` and `sizeBytes`.
The validator recomputes suite status/counts/ignored/skipped/formats from raw
logs and rejects missing, stale, modified, truncated, timeout, missing EOF,
duplicate, or multi-log suite evidence. Format coverage is accepted only from
the vertical suite.

## Status semantics

| Status | Meaning |
|---|---|
| `not_run` | Default / `MARKHAND_E2E!=1` |
| `fail` | Opted in but suites/matrix/provenance/F02/redaction incomplete |
| `pass` | `schemaVersion=2`, `MARKHAND_E2E=true` (bool), clean git tree, current full Git SHA/GitHub SHA binding, exact canonical suite commands and expected test names, one raw log per suite with harness exit/timeout/truncation/EOF records, full format matrix from workload YAML, black-box Compose API HTTP probes passed, structured external worker-kill/lease/reclaim/replay/DB proof, F02 project/container/image/hash match, provenance + redaction OK, no high/critical findings |

## Prerequisites

- POC Compose project (`MARKHAND_COMPOSE_PROJECT`, default `markhand-poc`) with
  expected services: api, postgres, minio, qdrant, worker-convert, worker-index
- F02 report with `passed=true`, `composeProject`, `containerIds`, `imageIds`
  and current migration/Compose/index provenance
- `MARKHAND_INDEX_SIGNATURE` = 64 lowercase hex (or readable from API container env)
- Postgres/MinIO/Qdrant URLs for tests (`MARKHAND_TEST_*`)
- API credential source: `MARKHAND_O04_API_BEARER_TOKEN`, or
  `MARKHAND_O04_API_EMAIL` + `MARKHAND_O04_API_PASSWORD`
- Existing foreign resource ids for cross-tenant denial:
  `MARKHAND_O04_FOREIGN_COLLECTION_ID` and `MARKHAND_O04_FOREIGN_DOCUMENT_ID`
- `cargo build -p fileconv-cli --no-default-features` → `target/debug/fileconv`
- Vision OCR config for PNG OCR: worker `MARKHAND_OCR_API_KEY` (OpenRouter
  default) or a self-hosted vision endpoint via `MARKHAND_OCR_BASE_URL`
  (ADR 0016 — Tesseract removed; missing OCR config ⇒ live fail, not skip)
- Clean git tree. Dirty release runs fail closed with `git_dirty`.

## Run

```bash
# Hermetic validator + command-shape negatives
bash deploy/scripts/o04-release-suite.sh --self-test

# Template evidence (honest not_run)
python3 bench/markhand_web/scripts/run_o04_release_suite.py

# Live release evidence
export MARKHAND_E2E=1
export MARKHAND_O04_EXTERNAL_WORKER_KILL=1
export MARKHAND_O04_WORKER_KILL_JOB_ID=<convert-job-currently-leased-by-poc-convert-1>
export MARKHAND_INDEX_SIGNATURE=...   # 64 lowercase hex
# plus MARKHAND_TEST_* from deploy/.env / contributor setup
bash deploy/scripts/o04-release-suite.sh

# Template/non-live Rust checks; does not require canonical pass
cargo test -p fileconv-server --test e2e_release_suite

# Release gate: fails unless canonical o04-release.json validates as pass
MARKHAND_RELEASE_GATE=1 cargo test -p fileconv-server --test e2e_release_suite -- --nocapture
# Equivalent wrapper
bash deploy/scripts/o04-release-suite.sh --release-gate --nocapture
```

`--release-gate` writes evidence outside the source tree by default
(`${TMPDIR}/markhand-o04-release-${GITHUB_SHA}` or `O04_OUTPUT_DIR`) and validates
that generated report through the Rust gate with `O04_REPORT_PATH`. CI uploads
that directory as the `phase1b-o04-release-<sha>` artifact. Do not validate a
committed `fail`/`not_run` fixture as the release gate.

## Expected formats

Loaded from `bench/markhand_web/workloads/phase1b-mixed.yaml` ingest formats
(currently: `csv`, `docx`, `html`, `pdf`, `png`, `pptx`, `txt`, `xlsx`).
Python harness and Rust vertical slice both parse this file — do not maintain
a second hard-coded list.

## Redaction

Raw logs and serialized reports are scanned for residual Cookie/Set-Cookie,
Basic auth, Bearer/session/capability/private keys, cloud keys, JWT, generic
password/token/secret/API/access keys, and URL-userinfo patterns. Findings
recompute `redactionScan.passed=false` and block `pass`.

## Current Live Blockers To Expect

The printable `O04_WORKER_HARD_KILL_EVIDENCE` line is no longer accepted.
Pass requires structured `externalWorkerKill` evidence showing a harness-owned
Docker kill/recreate of a distinct worker container plus death, lease expiry,
replacement reclaim/replay, and DB-state verification.

`MARKHAND_O04_WORKER_KILL_JOB_ID` must name a real convert job that is already
leased by `poc-convert-1` when the probe starts (override the expected worker
prefix with `MARKHAND_O04_WORKER_ID` only if the Compose worker id changes). The
harness queries POC Postgres through the Compose `postgres` container and only
sets `leaseExpired`, `replacementReclaimed`, `replayConsistent`, and
`dbStateVerified` from `jobs`, `event_log`, and `outbox_events`: leased pre-kill
row, expired killed lease, `job.reclaimed`, and final `job.succeeded` with no
active lease.
