<!-- generated-done-issue-plan: P1B-R02 -->
# P1B-R02 — Citation, preview và download authorization

Issue closed: 2026-07-31
Source issue: [#92](https://github.com/anhnth24/project-example/issues/92)
Catalog: [`backlog/phase-1b/issues/README.md`](../markhand-web/backlog/phase-1b/issues/README.md)
Phase plan: [`phase-1b-single-org-poc.md`](../markhand-web/phase-1b-single-org-poc.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Citation, preview và download authorization**.

## Context

- Phase: `1B`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> CI rust-integration SUCCESS on `b5cc92c` (run
> [30603158015](https://github.com/anhnth24/project-example/actions/runs/30603158015)/job
> [91070008980](https://github.com/anhnth24/project-example/actions/runs/30603158015/job/91070008980)):
> `live_citation_authz_expiry_replay_idor_and_immediate_deny` passed with
> worker-produced history/IDOR/delete paths; `live_minio_cleanup_guard_soak`
> passed. Prior multi-format vertical slice retained:
> `live_upload_convert_index_citation_vertical_slice` covers all
> `phase1b-mixed.yaml` ingest formats via HTTP upload → ConvertWorker/`fileconv`
> → IndexWorker → citation resolve on worker-produced IDs/artifacts/chunks.

## Implementation plan

Stable anchor pin logical document/version number/version ID/content hash/
effective time/current flag; fresh auth per resolve; trusted Markdown fetch; short
single-purpose download capability.

## Files/modules

`services/{access,citation,preview,download}.rs`, `routes/documents.rs`,
`migrations/0018_expand_download_capability_redemptions.sql`,
`tests/{citation_authz_matrix.rs,common/fixtures.rs}`.

## Dependencies / blocks

F05/F06/R01.

## Acceptance criteria

Quote/hash/version/anchor valid; historical permission + fresh
ACL; delete/suspend/removal deny; IDOR, expiry/replay, multi-document/multi-version,
PDF/PPTX/XLSX anchor tests.

## Required tests / evidence

Quote/hash/version/anchor valid; historical permission + fresh
ACL; delete/suspend/removal deny; IDOR, expiry/replay, multi-document/multi-version,
PDF/PPTX/XLSX anchor tests.

## Security and migration notes

No raw bucket credential/key.

## Out of scope

rich rendering.

## Delivery evidence

### Implementation PRs

- UNKNOWN — no implementation PR is cited in the catalog status.

### Recorded commit/SHA references

- `b5cc92c`

- GitHub sync-closed timestamp: `2026-07-31T06:23:21Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
