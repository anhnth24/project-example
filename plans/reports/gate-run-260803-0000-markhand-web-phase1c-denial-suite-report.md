# Phase 1C denial-suite gate run — TEMPLATE

> **This file is a skeleton, not a real run.** No `deployed-1c-integration` job
> has produced a report yet as of 2026-08-03 (see
> `plans/markhand-web/backlog/phase-1c/issues/README.md`, issues 1C-12/1C-13:
> "gate có tên KHÔNG tồn tại" before this infrastructure landed). Every bracketed
> field below is a placeholder — do not fill in numbers from memory or
> inference; copy this file to a new `gate-run-<YYMMDD-HHMM>-markhand-web-
> phase1c-denial-suite-report.md` and fill it in only from an actual CI run or
> local reproduction of the commands below. If a field cannot be verified from
> a real run, leave it as `[unverified]` rather than guessing.

Date: `[YYYY-MM-DD]`
Commit under test: `[full 40-char git sha]`
Branch: `[branch name]`
CI run: `[https://github.com/<org>/<repo>/actions/runs/<id>]` (job:
`deployed-1c-integration`)
Compose project: `[docker compose project name printed by deploy/scripts/poc-up.sh]`
Host: `[runner label, e.g. ubuntu-22.04 GitHub-hosted runner, or local host spec]`

## Gate identity

| Field | Value |
|---|---|
| Gate id(s) | `1C-12`, `1C-13` (`bench/markhand_web/gates.yaml`) |
| externalGate | `G0-SEC` |
| failureDisposition | `block-phase-1c` |
| environmentId | `poc-compose` |
| blocksIssues | `1C-12`, `1C-13` (Phase 1C exit gate; transitively `P2-16`, see `plans/markhand-web/backlog/phase-2/issues/README.md`) |

## Test summary

| Gate | Status | Tests run | Passed | Failed | Ignored/skipped |
|---|---|---:|---:|---:|---:|
| 1C-12 (multi-org denial suite) | `[pass/fail/not_run]` | `[n]` | `[n]` | `[n]` | `[n]` |
| 1C-13 (security/revoke/load gate) | `[pass/fail/not_run]` | `[n]` | `[n]` | `[n]` | `[n]` |

Command executed (deployed environment, `deployed-1c-integration` job):

```bash
cargo test -p fileconv-server --test '*' --no-fail-fast -- --ignored
```

CI half (same tests, `rust-integration` job against `deploy/dev/compose.yml`)
ran at commit `[sha]`, run `[url]` — record separately if it diverges from the
deployed-environment result above; the 1C-12/1C-13 acceptance criteria require
**both** CI and deployed to be green, not either alone.

## Per-test results

> Link each row to the actual test file/function once the connected suite
> (plan A1-A2 / B1-B7 in the phase-1c backlog) lands. Until then this table is
> empty by design — do not list the ~10 scattered `#[ignore]` cross-org checks
> here as if they were the "suite gắn kết" the 1C-12 issue calls for; that
> conflation was the exact gap the assessment flagged.

| Test | File | Result | Notes |
|---|---|---|---|
| `[test_name]` | `crates/server/tests/[file].rs` | `[pass/fail]` | `[note]` |

## Performance / load metrics (1C-13 only)

| Metric | Measured | Threshold | Source |
|---|---:|---:|---|
| `[metric name]` | `[value]` | `[operator] [value]` | `[script/report path]` |

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
cargo test -p fileconv-server --test '*' --no-fail-fast -- --ignored
docker compose -f deploy/compose.poc.yml down -v
```

## Artifact

CI artifact name: `deployed-1c-integration-<sha>` (contains
`1c-integration-report.md` + raw `test-output.log`, per the
`deployed-1c-integration` job in `.github/workflows/ci.yml`).
