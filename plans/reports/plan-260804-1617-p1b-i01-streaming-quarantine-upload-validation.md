<!-- generated-done-issue-plan: P1B-I01 -->
# P1B-I01 — Streaming quarantine upload validation

Date: 2026-08-04
Source issue: [#84](https://github.com/anhnth24/project-example/issues/84)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Streaming quarantine upload validation**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> Catalog records status as Done.

## Implementation plan

Multipart stream+hash; magic/extension canonical format; OOXML limits;
PDF/audio limits; retention disposition.

## Files/modules

`routes/uploads.rs`, `services/upload/{stream,sniff,archive,limits}.rs`.

## Dependencies / blocks

F04/F06 + G0-SEC/G0-CAP.

## Acceptance criteria

Spoof/bomb/oversize/malformed/traversal/interruption rejected
hoặc safely quarantined; bounded memory; adversarial/property tests.

## Required tests / evidence

Spoof/bomb/oversize/malformed/traversal/interruption rejected
hoặc safely quarantined; bounded memory; adversarial/property tests.

## Security and migration notes

Filename metadata only.

## Out of scope

resumable upload/malware service.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- UNKNOWN — no completion/evidence commit is cited in the catalog status.

- GitHub sync-closed timestamp: `2026-07-19T15:17:24Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
