# Task 13 Report — Unified multi-org denial coverage

**Branch:** `cursor/phase1c-denial-suite-6ddb`  
**Base:** `b3df83a`  
**Status:** GREEN_IMPLEMENTED — verification pending  
**Date:** 2026-08-01

## Commits

| SHA | Message |
|-----|---------|
| `b8d7b56` | `test(server): expose remaining multi-org denial gaps` |
| this GREEN commit | `test(server): complete unified multi-org denial coverage` |

## Formal RED evidence

CI run `30690584586`, job `91344530694`: five exact failures and one exact
pass (`duplicate_names_across_orgs_do_not_create_an_oracle`).

## GREEN implementation

- `WorkerPipeline::index_existing_document_revision` uploads a revision to each
  existing world document through production HTTP, then runs ConvertWorker,
  IndexWorker, and EmbeddingWorker with the exact caller permission set.
- Convert completion accepts designed reconciliation/acknowledgement outcomes
  only after the durable job status is `succeeded`.
- `IndexedDenialRuntime` keeps the mock embedding server, Qdrant collection,
  hermetic streaming chat provider, and retrieval router alive. Explicit
  teardown deletes Qdrant before world DB/MinIO cleanup; Drop provides
  panic-path best-effort Qdrant cleanup.
- Booted document version and trusted object key are refreshed from the
  worker-produced current version.
- Org-switch leakage needles are oriented independently: origin while warming,
  target after switching.
- Downgrade semantics preserve authentication but re-resolve to zero permissions
  and zero allowed collections; old and rotated tokens cannot list members.
  Suspend/remove still reject old access and refresh tokens with 401.
- Preview/download/job/SSE probes run against the live Qdrant/hermetic-provider
  router, so body-scope denial is 403 before retrieval rather than 503.
- In-flight ask obtains its session ID by bounded durable DB polling. The
  production ACL mutation records the committed event high-water under the
  principal authz lock; resume starts after that high-water and permits only a
  revocation terminal event, never post-revoke `ask.token` content.

## Manifest

All six Task 13 rows are executable and point to the exact
`multi_org_denial` tests. Counts: 79 total rows, 74 executable, 0 deferred,
5 N/A. Supplemental HTTP/SSE evidence is marked `secondary`; primary coverage
remains exactly 52 HTTP + 1 SSE for 53 business operations.

## Verification

Post-push checks pending. The Cloud VM has no live test service URLs, so CI is
authoritative for the ignored integration suite.
