# Phase 1C G1C-SEC Security Gate Runbook

Qualifying evidence for Phase 1C security/load gates (`G1C-SEC-*`) on the
`phase1c-multi-org-poc` profile. This harness is **opt-in** — it does not run on
every push.

## Triggers

- GitHub Actions: `workflow_dispatch` or PR label `run-phase1c-gate` (`phase1c-g1c-security-gate` job)
- Local (owner-approved hardware matching `bench/markhand_web/environments/phase1c-multi-org-poc.yaml`):

```bash
cp deploy/.env.example deploy/.env
deploy/scripts/poc-up.sh
deploy/scripts/poc-health.sh
export GITHUB_SHA="$(git rev-parse HEAD)"
MARKHAND_TEST_REQUIRED=1 MARKHAND_PHASE1C_GATE=1 \
  bash deploy/scripts/g1c-security-gate.sh --output-dir /tmp/markhand-phase1c-gate
```

Required inputs:
- `GITHUB_SHA` — exact source revision binding (defaults to `git rev-parse HEAD` in the shell orchestrator when unset)
- `MARKHAND_TEST_REQUIRED=1` and `MARKHAND_PHASE1C_GATE=1`
- `COMPOSE_PROFILES=mock`
- `MARKHAND_PHASE1C_OUTPUT_DIR` or `--output-dir` (absolute path)

## Preconditions

- `COMPOSE_PROFILES=mock` (local/mock embedding — cloud/shared profiles are rejected)
- `MARKHAND_TEST_REQUIRED=1`
- Dedicated worker DB URL on all qualifying workers (`MARKHAND_WORKER_DATABASE_URL`)
- AppArmor profile loaded: `sudo apparmor_parser -r -W deploy/poc/apparmor-markhand-convert`

## Deployed probe architecture (Task 16)

Qualifying PASS metrics come **only** from deployed probes in
`bench/markhand_web/scripts/phase1c_deployed_probes.py` wired through
`run_phase1c_gate.py`. There are **no** test-only HTTP endpoints, no
`test-hooks`, and no in-process Cargo tests as qualifying evidence.

| Subsystem | Source |
|-----------|--------|
| Cross-tenant denial | `phase1c_http_denial.py` black-box driver: all primary HTTP/SSE manifest rows mapped through guard inventory/OpenAPI paths; foreign/unauth scenarios; `leakageCount` from observed marker violations only |
| Revoke / ACL cache / stale tokens | Production auth + member PATCH/DELETE/refresh APIs |
| Quota recovery | Real upload + authoritative POC jobs SQL + worker lifecycle + `quota.reconcile` audit |
| Noisy neighbor | 60s concurrent uploads + 100 quiet-org search samples (canonical duration enforced) |
| Qdrant fail-closed | Compose stop/start single-node Qdrant; search/ask must fail closed with zero foreign markers |
| Audit coverage | Real admin mutations + `/audit` correlation (ratio computed, never hard-coded) |
| Worker role | Harness-supplied `MARKHAND_PHASE1C_WORKER_NONCE`; worker echoes exact nonce |
| Container vulns | Digest-pinned Trivy (no `--ignore-unfixed`); both API and worker reports validated |

**Seed boundary:** `deploy/scripts/phase1c-multi-org-seed.sh` delegates to
`bench/markhand_web/scripts/phase1c_multi_org_seed.py`. The primary POC org/admin
(`11111111-…` / `22222222-…201`) is fixture-bound in the POC database; the second
identity (`33333333-…301`, `phase1c-beta@poc.example`) is bootstrapped via controlled
POC DB setup because self-registration is unavailable. All org/collection/membership
relationships after bootstrap use production login/invite/accept/org-switch/collection APIs.
Tokens live only in `MARKHAND_PHASE1C_CREDENTIALS_JSON` (mode `0600`, purged after run);
public seed evidence contains hashes/IDs only and binds `sourceRevision` to `GITHUB_SHA`
and `manifestSha256` to canonical manifest bytes.

**CI substrate:** Cargo integration tests (`crates/server/tests/*` with
`PHASE1C_PROBE_RESULT`) and `scripts/run-phase1c-denial-suite.py` remain CI
substrate only. They do not qualify PASS alone.

## Transactional evidence

- Private temp staging dir; sanitize/residual-scan before commit
- Lock + atomic per-file renames; `phase-1c-gate.json` committed **last**
- Purge allowlisted artifacts before run and on any failure
- Symlink/path escape rejected in Python and shell entrypoints

## Trivy pin provenance

- **Release:** [aquasecurity/trivy v0.73.0](https://github.com/aquasecurity/trivy/releases/tag/v0.73.0) (2026-08-03)
- **Verification:** Docker Hub API `aquasec/trivy:0.73.0` linux/amd64 digest
- **Committed pin:** `deploy/poc/images.lock.json` → `images.trivy`
- **Compose usage:** `deploy/compose.poc.yml` service `trivy-scanner` (`COMPOSE_PROFILES=scan`)

```
aquasec/trivy:0.73.0@sha256:4bbf3824d974b70f27631005e2e6194d4d8fbd6e72c4a9e04cf521e25c5cb07f
```

Never use `:latest`, unpinned actions, or `curl | bash` for scanner install.

## Validation commands

```bash
python3 bench/markhand_web/scripts/test_phase1c_deployed_probes.py
python3 bench/markhand_web/scripts/test_run_phase1c_gate.py
python3 scripts/check-markhand-gates.py
python3 bench/markhand_web/scripts/run_phase1c_gate.py \
  --validate-report bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json
MARKHAND_PHASE1C_GATE=1 \
MARKHAND_PHASE1C_REPORT_PATH=bench/markhand_web/reports/phase-1c-gate/phase-1c-gate.json \
cargo test -p fileconv-server --test e2e_phase1c_gate -- --ignored e2e_phase1c_gate --nocapture
```

## Fail-closed behavior

- Missing/partial/malformed probe output → gate `fail`, no `pass` report
- Residual secrets in report/evidence → write rejected (`HarnessWriteError`)
- Path traversal/symlinks in evidence paths → rejected by Task 15 validators
- `targetMatch=false` with `status=pass` → rejected
- CI enforces harness result before artifact upload; failure uploads only
  `phase1c-gate-failure.json` (schema-valid diagnostic, never raw logs)

## No live evidence in repository

The committed template remains `status=not_run`. A qualifying `pass` report is produced
only by an opted-in live run (Task 17) and must not be committed until owner sign-off.
