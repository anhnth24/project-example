# P1B-O04 — Vertical-slice / security release suite

## Purpose

Machine-verifiable release evidence for the single-org POC vertical slice:

upload → convert/index → citation (all `phase1b-mixed.yaml` ingest formats), plus
unauthorized/cross-tenant denial, suspend/membership/delete deny, adversarial
upload reject/contain, and worker kill/replay consistency.

## Architecture (honest)

O04 suites are **cargo integration tests** that boot **in-process** axum +
ConvertWorker/IndexWorker against live PG/MinIO/Qdrant endpoints
(`MARKHAND_TEST_*`). They do **not** exercise HTTP against the Compose API
container. Evidence records `architecture.apiHttpExercised=false`.

Live pass also requires F02 machine report
`bench/markhand_web/reports/poc-f02-boot.json` with `passed=true` and matching
`composeProject` + `imageIds` for the POC Compose project.

## Evidence paths (O04 only)

| Artifact | Path |
|---|---|
| Report JSON | `bench/markhand_web/reports/phase-1b-gate/o04-release.json` |
| Report MD | `bench/markhand_web/reports/phase-1b-gate/o04-release.md` |
| Raw logs | `bench/markhand_web/reports/phase-1b-gate/raw/o04-<git>/` |

Do **not** use or overwrite O05 `summary.json`.

## Status semantics

| Status | Meaning |
|---|---|
| `not_run` | Default / `MARKHAND_E2E!=1` |
| `fail` | Opted in but suites/matrix/provenance/F02/redaction incomplete |
| `pass` | `MARKHAND_E2E=1`, every required suite exit 0 with testsRun>0, full format matrix from workload YAML, F02 boot match, provenance + redaction OK, no high/critical findings |

## Prerequisites

- POC Compose project (`MARKHAND_COMPOSE_PROJECT`, default `markhand-poc`) with
  expected services: api, postgres, minio, qdrant, worker-convert, worker-index
- `poc-f02-boot.json` with `passed=true`, `composeProject`, `imageIds`
- `MARKHAND_INDEX_SIGNATURE` = 64 lowercase hex (or readable from API container env)
- Postgres/MinIO/Qdrant URLs for tests (`MARKHAND_TEST_*`)
- `cargo build -p fileconv-cli --no-default-features` → `target/debug/fileconv`
- Tesseract + `vie+eng` for PNG OCR (missing OCR ⇒ live fail, not skip)

## Run

```bash
# Hermetic validator + command-shape negatives
bash deploy/scripts/o04-release-suite.sh --self-test

# Template evidence (honest not_run)
python3 bench/markhand_web/scripts/run_o04_release_suite.py

# Live release evidence
export MARKHAND_E2E=1
export MARKHAND_INDEX_SIGNATURE=...   # 64 lowercase hex
# plus MARKHAND_TEST_* from deploy/.env / contributor setup
bash deploy/scripts/o04-release-suite.sh

# Cargo gate (invokes Python --validate-report on o04-release.json)
cargo test -p fileconv-server --test e2e_release_suite
MARKHAND_E2E=1 cargo test -p fileconv-server --test e2e_release_suite -- --ignored --nocapture
```

## Expected formats

Loaded from `bench/markhand_web/workloads/phase1b-mixed.yaml` ingest formats
(currently: `csv`, `docx`, `html`, `pdf`, `png`, `pptx`, `txt`, `xlsx`).
Python harness and Rust vertical slice both parse this file — do not maintain
a second hard-coded list.

## Redaction

Raw logs are pattern-redacted. Residual password/token/JWT/URL-userinfo patterns
mark `redactionScan.passed=false` and block `pass`.
