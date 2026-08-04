<!-- generated-done-issue-plan: F-12 -->
# F-12 — Contributor workflow, setup docs và foundation gate

Date: 2026-08-04
Base commit: UNKNOWN — not recorded in the source catalog
Source issue: [#57](https://github.com/anhnth24/project-example/issues/57)
Catalog: [`backlog/phase-f/issues/README.md`](../markhand-web/backlog/phase-f/issues/README.md)
Phase plan: [`phase-f-engineering-foundation.md`](../markhand-web/phase-f-engineering-foundation.md)
Status: Done

## Objective

Chứng minh contributor mới có thể setup và tuân conventions.

## Context

- Phase: `F`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

ADR/RFC templates/index; ownership/CODEOWNERS; issue/PR
templates; Definition of Ready/Done; security triggers; setup/troubleshooting.

## Files/modules

`docs/adr/{README,TEMPLATE}.md`, `.github/CODEOWNERS`,
`.github/{ISSUE_TEMPLATE,PULL_REQUEST_TEMPLATE}.md`, contributor/runbook docs.

## Dependencies / blocks

F-01…F-11; blocks Phase 0/1A activation.

## Acceptance criteria

Clean-checkout onboarding không tribal knowledge; ownership/
approval rõ; Phase 0/1A không cần tạo convention riêng.

## Required tests / evidence

Independent setup dry run gồm pinned Node/pnpm/Rust/
task-runner/Compose/native prerequisites; full local/CI task cho app+web; dev stack;
sample contract/config/telemetry/fixture checks.

## Security and migration notes

Security review triggers và secret incident contact documented.

## Out of scope

Benchmark/business implementation/production runbooks.

## Delivery evidence

### Implementation PRs

- [PR #181](https://github.com/anhnth24/project-example/pull/181) — docs: add contributor setup and Phase F gate; merged `2026-07-17T14:28:28Z`

### Completion/evidence commits

- `29832476c6a775b56101301dbddcbaa69cc9ed7b`

- GitHub sync-closed timestamp: `2026-07-18T17:05:46Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
