# P1B-R06 hanging-dependency soak

Qualifies the readiness contract against the real POC Compose stack while a
dependency **hangs** instead of stopping. `dependency-outage.md` covers a
dependency that is down: the socket refuses and the API fails fast. A hung
dependency accepts the connection and never answers, so a request can park
forever — that is the case this soak measures, and the only one that proves the
outer probe deadline is real rather than nominal.

Hermetic coverage already exists (`readiness.rs` unit tests, the hanging-probe
router matrix). This gate exists because a hermetic hang is a stub returning
late; a paused container is the kernel holding the socket open.

## Preconditions

- Docker host with the POC stack up: `deploy/scripts/poc-up.sh` then
  `deploy/scripts/poc-health.sh`.
- `MARKHAND_HANGING_SOAK=1`. Without it the report is an honest `not_run` and
  cannot claim pass — same opt-in shape as `MARKHAND_SOAK` for O05.
- Not a CI job. Per-push CI runs the harness self-test only; see
  `docs/conventions/ci.md` on live gates being opt-in.

## Run

```bash
MARKHAND_HANGING_SOAK=1 deploy/scripts/r06-hanging-soak.sh
```

The harness waits for the expected hung `/ready` code after `docker pause`
before starting the 60s sustain window, so warm pool connections that still
return 200 briefly are recorded under `postPauseWarmup` rather than counted
against `readyCodeCorrect`.

Self-test only (no stack, runs anywhere, also wired into `make check-markhand-gates`):

```bash
deploy/scripts/r06-hanging-soak.sh --self-test
```

## What it asserts

For each dependency the readiness contract actually probes — `database`,
`vector_store`, `object_store`, `embedding`, matching
`ReadinessProbeError::code()` in `crates/server/src/services/readiness.rs` —
while that dependency is paused:

- `/api/v1/health/ready` answers **503 with the matching probe code**
  (`ready_database`, `ready_vector_store`, `ready_object_store`,
  `ready_embedding`), and answers **within the outer deadline** rather than
  parking for the length of the hang.
- Routes that touch no probe (`/api/v1/health/live`, `/api/v1/openapi.yaml`)
  keep answering inside their budget.
- Concurrent checkers stay bounded and do not grow without limit.
- Unpausing restores readiness, and the restore is confirmed rather than
  assumed.

Coverage is fail-closed: a run over a `--dependencies` subset is labeled
non-qualifying and cannot report pass.

## Reading the report

`bench/markhand_web/reports/phase-1b-gate/r06-hanging-soak.{json,md}` plus
`raw/r06-<stamp>/`. Status is `not_run` without the opt-in, `incomplete` when
evidence is partial, `fail` on a real gate breach, and `pass` only with zero
blockers. A `pass` carries provenance and a raw manifest so it can be
re-checked — the same rule the O05 report is now held to by
`crates/server/tests/e2e_release_suite.rs`.

## If it fails

- **503 but wrong probe code** — readiness attributed the hang to the wrong
  dependency. Check probe ordering in `readiness.rs`; a misattributed probe
  sends an operator to the wrong runbook.
- **Correct code but over the deadline** — the outer timeout is not bounding the
  inner probe. This is the defect the gate exists to catch.
- **`/health/live` or `/openapi.yaml` slow** — a dependency probe leaked into a
  route that must never touch one.
- **Restore not confirmed** — unpause did not bring readiness back; treat as a
  live incident and follow `dependency-outage.md`.
