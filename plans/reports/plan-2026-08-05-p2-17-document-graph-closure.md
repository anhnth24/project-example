# P2-17 — Document graph

Created: 2026-08-05
Source issue: catalog synchronization pending
Catalog: [Phase 2 issue catalog](../markhand-web/backlog/phase-2/issues/README.md#p2-17--document-graph)
Phase plan: [Phase 2 Web SPA](../markhand-web/phase-2-web-spa.md)
Status: Planned

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
- [PR #331](https://github.com/anhnth24/project-example/pull/331) — similarity edges.
- [PR #374](https://github.com/anhnth24/project-example/pull/374) — document deep link.

### Recorded commit/SHA references

- Graph MVP merge: `abb392099cfdd2df8427d26fee5ffb6ebc07ebd4`.
- Similarity merge: `0ae8105972f510a9a8d247fbd5fa3996ddcf60cc`.
- Deep-link merge: `2a8d7c053b0ca2288b0280511b0488cc2996db8a`.
- Closure branch commands, current SHA, review result, and final PR are pending execution.

## Definition of done

- [ ] Every acceptance row has current or exact-SHA reviewable evidence.
- [ ] Focused server/web/API checks pass.
- [ ] Qdrant integration evidence is verified and not a skipped/soft pass.
- [ ] Independent review finds no unresolved Critical/Important issue.
- [ ] Catalog text, status, roadmap, and tracker export are consistent.
- [ ] Plan and P2-17 are `Done` only after the preceding items pass.
