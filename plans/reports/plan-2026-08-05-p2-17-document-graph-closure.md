# P2-17 — Document graph

Created: 2026-08-05
Source issue: catalog synchronization pending
Catalog: [Phase 2 issue catalog](../markhand-web/backlog/phase-2/issues/README.md#p2-17--document-graph)
Phase plan: [Phase 2 Web SPA](../markhand-web/phase-2-web-spa.md)
Status: In progress

## Objective

Reconcile the stale P2-17 catalog body with the document-graph implementation already
merged, independently verify the server, Qdrant, web, and deep-link acceptance evidence,
and close only P2-17 when every applicable Definition of Done item is proven.

## Context

PR #327 delivered the graph API, ACL-scoped conflict/co-citation edges, deterministic
communities, web graph, mock data, accessibility fallback, and tests. PR #331 delivered
Qdrant recommend-by-point similarity edges with mandatory tenant/collection scope,
threshold/cap behavior, and a Qdrant-gated integration test. PR #374 completed the
graph-node-to-document-preview deep link.

The phase summary records those outcomes, but the P2-17 body still says similarity is a
stub. This closure must verify code and exact-SHA CI evidence rather than treating the
summary or merge state as proof. Batch recommend and corpus threshold tuning remain
explicitly out of scope.

## Implementation plan

1. Audit the authoritative issue, merged diffs, current server/web code, and test names.
2. Run the available focused server and web tests on the current branch.
3. Verify historical integration evidence for the Qdrant-backed path and tenant
   isolation from the exact implementation SHA.
4. If a real acceptance gap is found, add a failing regression test before the minimal
   implementation. Otherwise make no production-code change.
5. Correct the stale catalog text, record evidence, move P2-17 to `Review`, and
   regenerate roadmap/tracker artifacts.
6. Obtain independent security/code review. Move the plan and issue to `Done` only after
   all Critical/Important findings are resolved.

Failure behavior remains fail-soft when the vector index/embedder is unavailable, while
authorization and Qdrant scope remain fail-closed. No evidence may be inferred from a
skipped or unavailable integration test.

## Files/modules

- `crates/server/src/routes/graph.rs`: permission boundary and optional similarity deps.
- `crates/server/src/services/graph.rs`: bounded graph construction and similarity
  aggregation.
- `crates/server/src/storage/qdrant.rs`: scoped recommend-by-point adapter.
- `crates/server/tests/graph.rs`: PostgreSQL/Qdrant integration evidence.
- `web/src/pages/GraphPage.tsx`, `web/src/lib/forceLayout.ts`: graph UX and layout.
- `web/src/pages/GraphPage.test.tsx`, `web/e2e/graph.spec.ts`: UI and deep-link evidence.
- `plans/markhand-web/backlog/phase-2/issues/README.md`: authoritative status/evidence.
- Generated roadmap and tracker artifacts owned by `scripts/build-roadmap.py` and
  `scripts/sync-github-issues.py`.

## Dependencies / blocks

- P2-07 is `Done`; its library route accepts `?doc=<documentId>`.
- Phase 1B graph data sources and ask-stream persistence are merged.
- PRs #327, #331, and #374 are merged with successful relevant CI checks.
- No Phase 1C deployed qualification is required for this dev/test-gated Phase 2 UI
  issue. P2-17 does not close the Phase 2 exit gate.

## Acceptance criteria

| Criterion | Implementation location | Verification | Fixture/environment | Expected evidence |
|---|---|---|---|---|
| Graph API returns bounded visible nodes plus conflict/co-citation edges and deterministic communities | `routes/graph.rs`, `services/graph.rs`, `db/graph.rs` | focused unit tests and `tests/graph.rs` | hermetic unit; PostgreSQL integration | current command output plus PR #327 `rust-integration` |
| Graph access is org/ACL scoped and requires `qa.query` | route/service/repository graph path | permission, org-isolation, private-ACL integration cases | PostgreSQL integration | zero foreign node/edge assertions |
| Similarity edges use Qdrant recommend-by-point with mandatory org/collection filters, bounded fan-out, threshold, and cap | `storage/qdrant.rs`, `services/graph.rs` | aggregate/threshold/cap unit tests and `graph_similarity_edges_from_qdrant_recommend` | hermetic unit; Qdrant + PostgreSQL integration | PR #331 `rust` and `rust-integration` success on implementation SHA |
| Missing vector dependencies or Qdrant failure do not remove conflict/co-citation graph data | `routes/graph.rs`, `services/graph.rs` | fail-soft unit/integration assertions | hermetic unit and integration | focused test output; no broadened tenant scope |
| Web graph supports communities, collection filtering, table fallback, keyboard navigation, and node-to-document deep link | `GraphPage.tsx`, graph components, `forceLayout.ts` | Vitest and Playwright graph specs | mock web harness | current focused web tests plus PR #327/#374 web evidence |
| Public contract and generated client stay aligned | `openapi.yaml`, generated TypeScript contract | API drift checks | repository toolchain | `pnpm --dir web api:check` |

## Required tests / evidence

Run locally where supported:

```bash
cargo test -p fileconv-server services::graph --lib
pnpm --dir web test -- src/lib/forceLayout.test.ts src/pages/GraphPage.test.tsx
pnpm --dir web api:check
python3 scripts/build-roadmap.py --check
python3 scripts/sync-github-issues.py --dry-run
```

The DB/Qdrant test requires its configured integration environment:

```bash
cargo test -p fileconv-server --test graph -- --include-ignored --test-threads=1
```

When that environment is unavailable locally, verify rather than recreate the successful
PR #331 integration job for commit `0ae8105972f510a9a8d247fbd5fa3996ddcf60cc`.
Before any push containing Rust changes, also run the three mandatory Rust preflight
commands from `CLAUDE.md`.

## Security and migration notes

This issue crosses the tenant/ACL and Qdrant storage boundary, so independent security
review is mandatory. Review must confirm every recommend request carries mandatory
`VectorScope`, foreign-org points cannot become nodes/edges, vectors are not returned to
the application, and failures do not broaden scope. No migration or dependency change is
planned; if audit finds one necessary, stop and revise this plan before implementation.

## Out of scope

- Qdrant batch recommend.
- Corpus-based tuning of similarity threshold `0.5`.
- Graph clustering beyond deterministic connected components.
- Phase 2 production packaging, deployed SLO, or Phase 1C gate closure.
- Unrelated Help, Q&A, project, or chat-history work.

## Delivery evidence

### Implementation PRs

- [PR #327](https://github.com/anhnth24/project-example/pull/327) — graph MVP.
  Merge SHA `abb392099cfdd2df8427d26fee5ffb6ebc07ebd4` (2026-07-29).
  CI on merge SHA: `rust`, `rust-integration`, `web`, `web-e2e` SUCCESS
  (run [30435638525](https://github.com/anhnth24/project-example/actions/runs/30435638525)).
- [PR #331](https://github.com/anhnth24/project-example/pull/331) — similarity edges.
  Merge SHA `0ae8105972f510a9a8d247fbd5fa3996ddcf60cc` (2026-07-29).
  CI on merge SHA: `rust`, `rust-integration`, `web`, `web-e2e` SUCCESS
  (run [30446846876](https://github.com/anhnth24/project-example/actions/runs/30446846876)).
  `rust-integration` job started Postgres+Qdrant (`MARKHAND_TEST_QDRANT_URL=http://127.0.0.1:6333`)
  and logged `graph_similarity_edges_from_qdrant_recommend ... ok` (not skipped).
- [PR #374](https://github.com/anhnth24/project-example/pull/374) — document deep link.
  Merge SHA `2a8d7c053b0ca2288b0280511b0488cc2996db8a` (2026-08-03).
  CI on merge SHA: `rust`, `rust-integration`, `web`, `web-e2e` SUCCESS
  (run [30814514369](https://github.com/anhnth24/project-example/actions/runs/30814514369));
  `web-e2e` logged all three `e2e/graph.spec.ts` cases including `?doc=` deep-link.

### Audit outcome (closure branch)

- Base / worktree SHA at audit start: `019c2fe2070f9441e92e4dcf8b4f5fcf19fc9da0`
  (`docs(p2-17): add document graph closure plan`); working tree clean.
- Code reality: similarity is **not** a stub. `QdrantClient::recommend` +
  `services::graph::compute_similarity_edges` are live; route passes
  `SimilarityDeps` when vector index + embedder are configured; fail-soft on Qdrant
  error preserves conflict/co_citation; OpenAPI `GraphEdge.kind` enum includes
  `similarity`. No production-code change required for this closure.
- Local environment: Docker unavailable → PostgreSQL/Qdrant integration suite not
  re-run here; exact-SHA CI evidence above substitutes (not soft-passed).

### Local / hermetic commands (closure branch `cursor/p2-17-document-graph-e9d6`)

| Command | Result |
|---|---|
| `cargo test -p fileconv-server services::graph --lib` | **18 passed**, 0 failed |
| `pnpm --dir web exec vitest run src/lib/forceLayout.test.ts src/pages/GraphPage.test.tsx` | **2 files / 17 passed**, 0 failed |
| `pnpm --dir web api:check` | **pass** (no contract drift) |
| `cargo test -p fileconv-server storage::qdrant --lib` | **10 passed**, 0 failed (scope fail-closed helpers) |
| `cargo test -p fileconv-server --test graph -- --include-ignored --test-threads=1` | **not run locally** (no Docker/Postgres/Qdrant); verified via PR #331 CI log above |
| `python3 scripts/build-roadmap.py` | **pass** — 116 issues, status `{done:79, in_progress:7, review:1, backlog:29}` |
| `python3 scripts/build-roadmap.py --check` | **pass** — roadmap up to date, source `14e2121602531a1f` |
| `python3 scripts/sync-github-issues.py --export-json plans/markhand-web/backlog/github-issues.json` | **pass** — P2-17 `status: review` only |
| `python3 scripts/sync-github-issues.py --dry-run` | **pass** — `[2] P2-17 — Document graph (review)` |
| `python3 scripts/check-dependency-policy.py` | **pass** (no Rust source edits) |
| `cargo metadata --locked --format-version 1 --no-deps` | **pass** |

### Recorded commit/SHA references

- Graph MVP merge: `abb392099cfdd2df8427d26fee5ffb6ebc07ebd4`.
- Similarity merge: `0ae8105972f510a9a8d247fbd5fa3996ddcf60cc`.
- Deep-link merge: `2a8d7c053b0ca2288b0280511b0488cc2996db8a`.
- Closure plan commit: `019c2fe2070f9441e92e4dcf8b4f5fcf19fc9da0`.
- Closure evidence/catalog commit: this change set on `cursor/p2-17-document-graph-e9d6`.
- Independent review: pending (plan stays `In progress`; catalog → `Review`, not `Done`).

## Definition of done

- [x] Every acceptance row has current or exact-SHA reviewable evidence.
- [x] Focused server/web/API checks pass.
- [x] Qdrant integration evidence is verified and not a skipped/soft pass.
- [ ] Independent review finds no unresolved Critical/Important issue.
- [x] Catalog text, status, roadmap, and tracker export are consistent.
- [ ] Plan and P2-17 are `Done` only after the preceding items pass.
