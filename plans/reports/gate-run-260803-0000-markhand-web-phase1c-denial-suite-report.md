# Phase 1C denial-suite gate run — TEMPLATE

> **This file is a skeleton, not a real run.** No `deployed-1c-integration` job
> has produced a report yet as of 2026-08-03. Every bracketed field below is a
> placeholder — do not fill in numbers from memory or inference; copy this file
> to a new `gate-run-<YYMMDD-HHMM>-markhand-web-phase1c-denial-suite-report.md`
> and fill it in only from an actual CI run or local reproduction of the commands
> below. If a field cannot be verified from a real run, leave it as
> `[unverified]` rather than guessing.

Date: `[YYYY-MM-DD]`
Commit under test: `[full 40-char git sha]`
Branch/ref: `[branch or tag name — must match trusted --expected-git-ref]`
CI run: `[https://github.com/anhnth24/project-example/actions/runs/<id>]` (job:
`deployed-1c-integration`)
Compose project: `[docker compose project name printed by deploy/scripts/poc-up.sh]`
Host: `[runner label, e.g. ubuntu-22.04 GitHub-hosted runner, or local host spec]`

## Gate identity

| Field | Value |
|---|---|
| Gate id(s) | `1C-12` (`bench/markhand_web/gates.yaml`); `1C-13` explicitly `not_run` in artifact |
| externalGate | `G0-SEC` |
| failureDisposition | `block-phase-1c` |
| environmentId | `poc-compose` |
| blocksIssues | `1C-12`, `1C-13` (Phase 1C exit gate; transitively `P2-16`, see `plans/markhand-web/backlog/phase-2/issues/README.md`) |

## Test summary

| Gate | Status | Executable | N/A | Deferred | Leakage | Redaction | Runner | Teardown |
|---|---|---:|---:|---:|---:|---|---:|---:|
| 1C-12 (multi-org denial suite) | `[pass/fail/not_run]` | `[n]` | `[n]` | `[n]` | `[n]` | `[pass/fail]` | `[exit]` | `[exit]` |
| 1C-13 (security/revoke/load gate) | `not_run` | — | — | — | — | — | — | — |

Command executed (deployed environment, `deployed-1c-integration` job):

```bash
export MARKHAND_TEST_REQUIRED=1
export MARKHAND_1C_OUTPUT_DIR="${RUNNER_TEMP:-/tmp}/markhand-1c-integration"
mkdir -p "$MARKHAND_1C_OUTPUT_DIR"
PHASE1C_MANIFEST_SHA256="$(python3 -c "import hashlib; from pathlib import Path; print(hashlib.sha256(Path('crates/server/tests/fixtures/multi-org-denial.manifest.json').read_bytes()).hexdigest())")"
cargo build -p fileconv-cli --no-default-features
python3 scripts/run-phase1c-denial-suite.py \
  --manifest crates/server/tests/fixtures/multi-org-denial.manifest.json \
  --output "$MARKHAND_1C_OUTPUT_DIR/manifest-run.json"
RUNNER_EXIT=$?
docker compose -f deploy/compose.poc.yml down -v
TEARDOWN_EXIT=$?
python3 scripts/render-phase1c-denial-report.py \
  --input "$MARKHAND_1C_OUTPUT_DIR/manifest-run.json" \
  --output "$MARKHAND_1C_OUTPUT_DIR/phase1c-denial-report.md" \
  --expected-git-sha "$(git rev-parse HEAD)" \
  --expected-manifest-sha256 "$PHASE1C_MANIFEST_SHA256" \
  --expected-git-ref "$(git branch --show-current)" \
  --ci-run-url "https://github.com/anhnth24/project-example/actions/runs/[id]" \
  --runner-exit-code "$RUNNER_EXIT" \
  --teardown-exit-code "$TEARDOWN_EXIT"
```

CI half (same manifest runner, `rust-integration` job against
`deploy/dev/compose.yml`) ran at commit `[sha]`, run `[url]` — record separately
if it diverges from the deployed-environment result above; the 1C-12/1C-13
acceptance criteria require **both** CI and deployed to be green, not either
alone.

## Export surface (1C-12 acceptance item)

| Item | Status | Evidence |
|---|---|---|
| `export.run` / export HTTP operation | N/A-until-surface-exists | `na-export-route-absent` in `multi-org-denial.manifest.json`; `multi-org-denial.na-evidence.json` documents OpenAPI/guard-inventory absence |

Any future export operation must enter guard inventory and the denial manifest
before release.

## Per-test results

> Copy executable rows from `manifest-run.json` (`binariesRun`, `failures`) and
> the connected suite tests under `crates/server/tests/multi_org_denial.rs` once
> a live run succeeds. Do not fabricate counts.

| Test / binary | Result | Notes |
|---|---|---|
| `[binary or test]` | `[pass/fail]` | `[note]` |

## Performance / load metrics (1C-13 only)

| Metric | Measured | Threshold | Source |
|---|---:|---:|---|
| — | — | — | Gate `1C-13` remains `not_run` in this job |

## Known issues / exceptions

- `[List any waived checks, open ADRs, or accepted gaps here, each with owner
  and expiry per docs/adr/TEMPLATE.md's "Exception lifecycle" convention.]`
- External pentest (1C-13 "Out: external pentest" per the backlog) is out of
  scope for this job; record its own evidence path here once commissioned.

## Reproduction

```bash
cp deploy/.env.example deploy/.env
deploy/scripts/poc-up.sh
deploy/scripts/poc-health.sh
export MARKHAND_TEST_REQUIRED=1
export MARKHAND_1C_OUTPUT_DIR="${RUNNER_TEMP:-/tmp}/markhand-1c-integration"
mkdir -p "$MARKHAND_1C_OUTPUT_DIR"
export MARKHAND_TEST_DATABASE_URL=postgresql://markhand:markhand_poc_change_me@127.0.0.1:54330/markhand
export MARKHAND_TEST_APP_DATABASE_URL=postgresql://markhand_app:markhand_app_poc_change_me@127.0.0.1:54330/markhand
export MARKHAND_TEST_MINIO_ENDPOINT=http://127.0.0.1:9010
export MARKHAND_TEST_MINIO_ACCESS_KEY=markhand_app
export MARKHAND_TEST_MINIO_SECRET_KEY=markhand_app_poc_change_me
export MARKHAND_TEST_MINIO_REGION=us-east-1
export MARKHAND_TEST_QDRANT_URL=http://127.0.0.1:6343
export MARKHAND_TEST_QDRANT_ADMIN_API_KEY=test-operator-admin-key
PHASE1C_MANIFEST_SHA256="$(python3 -c "import hashlib; from pathlib import Path; print(hashlib.sha256(Path('crates/server/tests/fixtures/multi-org-denial.manifest.json').read_bytes()).hexdigest())")"
cargo build -p fileconv-cli --no-default-features
python3 scripts/run-phase1c-denial-suite.py \
  --manifest crates/server/tests/fixtures/multi-org-denial.manifest.json \
  --output "$MARKHAND_1C_OUTPUT_DIR/manifest-run.json"
RUNNER_EXIT=$?
docker compose -f deploy/compose.poc.yml down -v
TEARDOWN_EXIT=$?
python3 scripts/render-phase1c-denial-report.py \
  --input "$MARKHAND_1C_OUTPUT_DIR/manifest-run.json" \
  --output "$MARKHAND_1C_OUTPUT_DIR/phase1c-denial-report.md" \
  --expected-git-sha "$(git rev-parse HEAD)" \
  --expected-manifest-sha256 "$PHASE1C_MANIFEST_SHA256" \
  --expected-git-ref "$(git branch --show-current)" \
  --ci-run-url "https://github.com/anhnth24/project-example/actions/runs/local-repro" \
  --runner-exit-code "$RUNNER_EXIT" \
  --teardown-exit-code "$TEARDOWN_EXIT"
```

## Artifact

CI artifact name: `deployed-1c-integration-<sha>` (contains sanitized
`manifest-run.json` + `phase1c-denial-report.md` only, per the
`deployed-1c-integration` job in `.github/workflows/ci.yml`). Raw cargo output is
not uploaded.
