<!-- generated-done-issue-plan: P1B-I04 -->
# P1B-I04 — Isolated converter worker

Issue closed: 2026-07-19
Source issue: [#87](https://github.com/anhnth24/project-example/issues/87)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Isolated converter worker**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Download quarantine; materialize server-derived canonical extension;
process/cgroup limits and kill descendants; ephemeral cleanup/heartbeat/cancel.

## Files/modules

`src/workers/{convert,sandbox,limits}.rs`, worker image/config.

## Dependencies / blocks

F02/I03 + G0-SEC/G0-CAP/G0-LIC.

## Acceptance criteria

No network/host FS; timeout kills tree; cleanup all outcomes;
fork/disk/RAM/malformed/cancel/all-format smoke.

## Required tests / evidence

No network/host FS; timeout kills tree; cleanup all outcomes;
fork/disk/RAM/malformed/cancel/all-format smoke.

## Security and migration notes

Unapproved model excluded, narrow credentials.

## Out of scope

VM sandbox.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:32Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
