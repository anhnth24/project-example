# Task 13 Report — RED: indexed/cache/stale-token denial gaps

**Branch:** `cursor/phase1c-denial-suite-6ddb`  
**Base:** `b3df83a`  
**Status:** RED_CHECKPOINT  
**Date:** 2026-08-01

## Commit

| SHA | Message |
|-----|---------|
| _(pending push)_ | `test(server): expose remaining multi-org denial gaps` |

## Contracts implemented

| Test | Contract | Expected RED | Rationale |
|------|----------|--------------|-----------|
| `indexed_fts_and_ask_never_return_foreign_marker` | Both org markers indexed via production `WorkerPipeline`; actor `/search` and `/ask` return own marker, never foreign IDs/keys/markers in body or headers | **FAIL** | `require_worker_indexed_orgs` asserts Task 12 world documents remain `converted` with zero `chunks` rows — no worker-produced FTS artifacts yet |
| `duplicate_names_across_orgs_do_not_create_an_oracle` | Shared display names list only actor-org IDs/counts; foreign path IDs → 404; foreign vs ghost UUID denial envelopes match (no existence oracle) | **PASS** | Uses existing SQL-seeded world + production list/get routes; no indexing prerequisite |
| `org_switch_never_reuses_previous_org_cache_scope` | Bridge user warms cache, `POST /orgs/switch` mints new session; post-switch collections exclude previous-org IDs; foreign path + body-scope search deny without leak | **PASS** | Production switch + `AuthenticatedOrg` cache path; test-local cross-org bridge user only |
| `pre_revoke_tokens_fail_after_downgrade_suspend_and_remove` | Tokens minted at boot fail on both `/auth/me` (access) and `/auth/refresh` after production PATCH downgrade/suspend and DELETE remove | **PASS** (verify live) | Mirrors `members.rs` refresh contracts plus access-token stale window; uses world pre-revoke tokens |
| `preview_download_job_and_sse_hide_foreign_ids` | Foreign preview, download-capability, job, job-events SSE, and cross-tenant capability redeem → 404 IDOR; body-scope search with foreign `collectionIds` → 403 | **PASS** | World exposes foreign job/document/version IDs + MinIO keys; denial paths do not need indexed chunks |
| `in_flight_ask_emits_no_content_after_acl_revoke` | Production `/ask/stream`, then `acl_mutate::revoke_collection_access_for_principal`; tail emits `citation_revoked`/`principal_denied`, no `ask.token` or foreign markers | **FAIL** | Blocked by same `require_worker_indexed_orgs` prerequisite before stream can ground |

## Verification (local VM)

| Check | Result |
|-------|--------|
| `cargo test -p fileconv-server --test multi_org_denial --no-run` | **OK** (compiles) |
| `cargo test -p fileconv-server --test multi_org_denial_manifest` | **15 passed** |
| `cargo test -p fileconv-server --test multi_org_denial -- --include-ignored` | Soft-skip (no `MARKHAND_TEST_DATABASE_URL` in Cloud VM) |
| `cargo fmt --all -- --check` | **OK** after format |

## Manifest

Deferred rows **not** promoted (RED only). Six `task13:*` guard refs remain `status: deferred`.

## Concerns

- **Indexed/in-flight RED depends on live DB**: failure message is explicit (`state=converted`, `0 chunks`) rather than `todo!` or compile errors; GREEN must wire `WorkerPipeline` indexing for both org markers without test-only production bypasses.
- **Pre-revoke access-token leg**: `members.rs` already proves refresh revocation; this test adds `/auth/me` access-token denial — if production allows JWT until expiry without membership re-check, this contract may flip from expected PASS to FAIL (would be a real gap).
- **Ask stream in-flight test** needs Qdrant when live; duplicates `sse_stream_readiness` ACL-revoke pattern but binds to `MultiOrgDenialWorld` + indexed prerequisite.
- **Org-switch bridge user** is test-local seeding (allowed in RED); GREEN may fold into world builder to avoid per-test membership setup.
- Cloud VM lacks live Postgres/MinIO/Qdrant — full `--include-ignored` RED run must happen in CI or contributor environment per task brief Step 2.
