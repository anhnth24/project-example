<!-- generated-done-issue-plan: P1B-F06 -->
# P1B-F06 — Fail-closed PG/Qdrant/MinIO adapters

Date: 2026-08-04
Source issue: [#83](https://github.com/anhnth24/project-example/issues/83)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Fail-closed PG/Qdrant/MinIO adapters**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Pools, opaque key builder, quarantine/trusted namespace, deterministic
points, versioned collection, mandatory org/collection filters, typed errors.

## Files/modules

`src/storage/{keys,minio,qdrant}.rs`, `src/db/pool.rs`,
`services/index_signature.rs`.

## Dependencies / blocks

F02/F04 + G0-ARCH/G0-RET/G1A.

## Acceptance criteria

Missing/empty filter rejected; no filename in key; payload has
all identities; real-service contracts, traversal/fuzz, deterministic vectors.

## Required tests / evidence

Missing/empty filter rejected; no filename in key; payload has
all identities; real-service contracts, traversal/fuzz, deterministic vectors.

## Security and migration notes

No public key, least privilege.

## Out of scope

generic backend trait.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:21Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
