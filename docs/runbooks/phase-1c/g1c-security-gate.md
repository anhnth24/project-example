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
MARKHAND_TEST_REQUIRED=1 MARKHAND_PHASE1C_GATE=1 \
  bash deploy/scripts/g1c-security-gate.sh --output-dir /tmp/markhand-phase1c-gate
```

## Preconditions

- `COMPOSE_PROFILES=mock` (local/mock embedding — cloud/shared profiles are rejected)
- `MARKHAND_TEST_REQUIRED=1`
- Dedicated worker DB URL on all qualifying workers (`MARKHAND_WORKER_DATABASE_URL`)
- AppArmor profile loaded: `sudo apparmor_parser -r -W deploy/poc/apparmor-markhand-convert`

## What the harness proves

1. Two orgs seeded via production HTTP APIs (`deploy/scripts/phase1c-multi-org-seed.sh`)
2. Canonical PR4 denial runner (`scripts/run-phase1c-denial-suite.py`)
3. Membership/ACL revoke, quota recovery, noisy-neighbor, audit coverage via deployed DB probes
4. Worker runtime role via `docker compose exec worker-convert fileconv-worker --db-role-probe`
   (queries `pg_roles`/`current_user` through the worker DB pool — not compose-text proof)
5. Container vulnerability scan via digest-pinned Trivy
6. Sanitized `phase-1c-gate.json` + ten allowlisted evidence files

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

## No live evidence in repository

The committed template remains `status=not_run`. A qualifying `pass` report is produced
only by an opted-in live run (Task 17) and must not be committed until owner sign-off.
