<!-- generated-done-issue-plan: P1A-10 -->
# P1A-10 — CI parity và extraction gate

Issue closed: 2026-07-18
Source issue: [#77](https://github.com/anhnth24/project-example/issues/77)
Catalog: [`backlog/phase-1a/issues/README.md`](../markhand-web/backlog/phase-1a/issues/README.md)
Phase plan: [`phase-1a-knowledge-extraction.md`](../markhand-web/phase-1a-knowledge-extraction.md)
Status: Done

## Objective

Chứng minh desktop equivalence và server usability.

## Context

- Phase: `1A`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> merged to `master`; full local extraction gate passed.

## Implementation plan

Full feature/contract/golden matrix; no-feature server consumer test;
dependency deny-list; docs compatibility; file perf/concurrency defects riêng.

## Files/modules

CI, `crates/knowledge/tests/`, desktop integration tests,
architecture/compatibility docs.

## Dependencies / blocks

Adapter cutover.

## Acceptance criteria

Tất cả test xanh; golden trong tolerance; IPC unchanged; legacy
index path tested; server consumer không desktop deps.

## Required tests / evidence

`cargo test` core/knowledge/desktop, `cargo tree`, `pnpm test/build`.

## Security and migration notes

Synthetic fixtures; explicit index rebuild notice.

## Out of scope

Server/storage/auth.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-18T17:06:27Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
