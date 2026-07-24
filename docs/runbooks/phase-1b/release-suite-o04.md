# P1B-O04 — Vertical-slice / security release suite

## Purpose

Machine-verifiable release evidence for the single-org POC vertical slice:

upload → convert/index → citation (all expected document formats), plus
unauthorized/cross-tenant denial, suspend/membership/delete deny, adversarial
upload reject/contain, and worker kill/replay consistency.

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
| `fail` | Opted in but suites/matrix/provenance/redaction incomplete |
| `pass` | `MARKHAND_E2E=1`, every required suite exit 0 with testsRun>0, full format matrix, provenance + redaction OK, no high/critical findings |

## Prerequisites

- POC or dev stack with Postgres (`MARKHAND_TEST_DATABASE_URL` +
  `MARKHAND_TEST_APP_DATABASE_URL`), MinIO, Qdrant
- `cargo build -p fileconv-cli --no-default-features` → `target/debug/fileconv`
- Docker available for version/image provenance capture

## Run

```bash
# Hermetic validator unit tests
bash deploy/scripts/o04-release-suite.sh --self-test

# Template evidence (honest not_run)
python3 bench/markhand_web/scripts/run_o04_release_suite.py

# Live release evidence
export MARKHAND_E2E=1
# plus MARKHAND_TEST_* / MinIO / Qdrant env from deploy/.env or contributor setup
bash deploy/scripts/o04-release-suite.sh

# Cargo gate (reads o04-release.json only)
cargo test -p fileconv-server --test e2e_release_suite
MARKHAND_E2E=1 cargo test -p fileconv-server --test e2e_release_suite -- --ignored --nocapture
```

## Expected formats

Explicit set (fixtures without OCR/audio models):

`csv`, `docx`, `html`, `pdf`, `pptx`, `txt`, `xlsx`

## Redaction

Raw logs are pattern-redacted. Residual password/token/JWT/URL-userinfo patterns
mark `redactionScan.passed=false` and block `pass`.
