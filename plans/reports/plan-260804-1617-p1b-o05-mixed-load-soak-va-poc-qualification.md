<!-- generated-done-issue-plan: P1B-O05 -->
# P1B-O05 — Mixed-load soak và POC qualification

Date: 2026-08-04
Source issue: [#101](https://github.com/anhnth24/project-example/issues/101)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Mixed-load soak và POC qualification**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> the official 1800s run passed every gate on 2026-07-26 at
> `f4f33cd`, on a 24-core Ubuntu host, Compose project
> `markhand-poc-f02-20260726t121843z-1815269-17292`, with F02/O01/O02/O03/O04
> passing on that same commit and project. `o05-soak.json` is `status=pass` with
> no blockers; report sha256 prefix `a1a6d0e6ee57df4d`.

## Implementation plan

Concurrent ingest/query/delete/reconcile against POC API per
`phase1b-mixed.yaml`; opt-in worker-kill/dependency blip; Docker/API/PG sampling;
evaluate binding thresholds from profile/gates/SLA; post-restore retrieval check.

## Files/modules

`bench/markhand_web/soak/*`, `workloads/phase1b-mixed.yaml`,
`reports/phase-1b-gate/o05-soak.*`, `docs/runbooks/phase-1b/soak-o05.md`,
`deploy/scripts/o05-soak.sh`.

## Dependencies / blocks

O02/O03/O04 + G0-CAP/G0-SLO.

## Acceptance criteria

Unit/self-test (fake OOXML/PDF/PNG fail preflight, compare
without dataset non-pass, async injection, partial injection counts fail,
restored==blue/missing non-pass, retained absent / unauthorized 2xx non-pass,
smoke≠pass); live: query p95≤500 / p99≤1000, ingest≥300 docs/h on
`poc-compose`, RSS≤256MB / temp≤512MB / queue≤100 / DB conn≤40, recovery +
green post-restore; duration exactly 1800.

## Required tests / evidence

Unit/self-test (fake OOXML/PDF/PNG fail preflight, compare
without dataset non-pass, async injection, partial injection counts fail,
restored==blue/missing non-pass, retained absent / unauthorized 2xx non-pass,
smoke≠pass); live: query p95≤500 / p99≤1000, ingest≥300 docs/h on
`poc-compose`, RSS≤256MB / temp≤512MB / queue≤100 / DB conn≤40, recovery +
green post-restore; duration exactly 1800.

## Security and migration notes

Synthetic/redacted, exact git/image/migration/index
versions; injection only on expected POC project/services.

## Out of scope

production/multi-org.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `a1a6d0e6ee57df4d`
- `f4f33cd`

- GitHub sync-closed timestamp: `2026-07-26T14:40:22Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
