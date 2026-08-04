# Phase 1C denial-suite gate run — 2026-08-03 (deployed half PASS)

Committed evidence from the first successful `deployed-1c-integration` live run.
Sanitized JSON is byte-identical to the CI artifact
(`manifest-run.json` from run 30849375921 / job 91805590040).

## Provenance

| Field | Value |
| --- | --- |
| Gate | `1C-12` (deployed environment half) |
| Environment | `poc-compose` |
| CI run | https://github.com/anhnth24/project-example/actions/runs/30849375921 |
| CI job | https://github.com/anhnth24/project-example/actions/runs/30849375921/job/91805590040 (`deployed-1c-integration`) |
| Started (UTC) | 2026-08-03T20:16:04Z |
| Completed (UTC) | 2026-08-03T20:49:11Z |
| Branch / ref | `cursor/phase1c-deployed-evidence-06b6` |
| Branch head at run | `bae0f585c2a84d2222b966f8dc620101a68d72f9` |
| Tested merge SHA (artifact `gitShaFull`) | `67d27b7ced3c04f25c62f299ed9a50be95009b47` |
| Manifest SHA-256 | `99a5af67d78f6712daa40b7f582f76ee0d53d334e13156daabeacd21978d9630` |
| Evidence JSON | `plans/reports/gate-run-260803-2049-markhand-web-phase1c-denial-suite-report.json` |

## Gate identity

| Field | Value |
|---|---|
| Gate id(s) | `1C-12` (`bench/markhand_web/gates.yaml`); `1C-13` explicitly `not_run` in artifact |
| externalGate | `G0-SEC` |
| failureDisposition | `block-phase-1c` |
| environmentId | `poc-compose` |
| blocksIssues | `1C-12`, `1C-13` (Phase 1C exit gate) |

## Test summary (deployed half)

| Gate | Status | Executable | N/A | Deferred | Binaries | Leakage | Redaction | Runner | Teardown | Failures |
|---|---|---:|---:|---:|---:|---:|---|---:|---:|---:|
| 1C-12 (multi-org denial suite) | **PASS** | 74 | 5 | 0 | 15 | 0 | pass | 0 | 0 | 0 |
| 1C-13 (security/revoke/load gate) | `not_run` | — | — | — | — | — | — | — | — | — |

## CI half caveat (1C-12 not Done)

The same labeled run's regular `rust-integration` job **flaked** on this attempt.
Prior run [30843983333](https://github.com/anhnth24/project-example/actions/runs/30843983333)
passed the CI half. **1C-12 remains In progress** until CI-half stability and root
cause are resolved; this report records only the deployed-environment half.

Phase 1C is **not closed** — exit gate still requires 1C-12 green on **both** CI and
deployed environment, plus 1C-13 on both halves.

## MinIO test fixture boundary (not IAM least-privilege evidence)

Integration tests create/delete ephemeral `markhand-it-*` buckets via
`test_minio_client()`. The deployed job sets `MARKHAND_TEST_MINIO_ACCESS_KEY` /
`MARKHAND_TEST_MINIO_SECRET_KEY` to the POC **root fixture** credentials from
`deploy/.env.example` (`MARKHAND_MINIO_ROOT_USER` / `MARKHAND_MINIO_ROOT_PASSWORD`).
Application and worker containers in `deploy/compose.poc.yml` continue to use the narrow
`markhand_app` key scoped to `MARKHAND_MINIO_BUCKET` only. Root in the test harness is
**bootstrap/ephemeral-bucket lifecycle only** — it does not prove MinIO IAM
least-privilege enforcement for runtime services.

---

# Phase 1C denial gate report — 1C-12

## Run identity

| Field | Value |
| --- | --- |
| Gate | `1C-12` |
| Environment | `poc-compose` |
| Git SHA (full) | `67d27b7ced3c04f25c62f299ed9a50be95009b47` |
| Git ref | `cursor/phase1c-deployed-evidence-06b6` |
| CI run | https://github.com/anhnth24/project-example/actions/runs/30849375921 |
| Manifest SHA-256 | `99a5af67d78f6712daa40b7f582f76ee0d53d334e13156daabeacd21978d9630` |

## Manifest counts

| Metric | Count |
| --- | ---: |
| Executable | 74 |
| N/A | 5 |
| Deferred | 0 |

## Execution

- Binaries run: acl_cache, api_http_contracts, audit_read, chat_history, citation_authz_matrix, direct_service_authz, graph, jobs, members, multi_org_denial, orgs, projects, repositories, sse_stream_readiness, storage
- Runner exit code: 0
- Teardown exit code: 0
- Leakage findings: 0
- Redaction scan: passed

## Failures

- (none)

## Related gates

- `1C-13`: `not_run` (this job does not measure load/revoke/fairness thresholds)

## Verdict

**PASS**

## Reproduction

See `plans/reports/gate-run-260803-0000-markhand-web-phase1c-denial-suite-report.md`
for the reusable command sequence and local POC reproduction steps.
