# ADR 0014: Vietnamese word segmentation for FTS lexical retrieval

- Status: Proposed
- Date: 2026-07-22
- Decision key: `vietnamese-word-segmentation-fts`
- Owners: retrieval-owner, architecture-owner
- Approver: Phase 1B architecture gate
- Related issues/PRs: P1B-R01; migration `0016_expand_chunks_accent_fold_tsv.sql`;
  ADR 0006, ADR 0007

## Context

The server hybrid retrieval lexical arm (P1B-R01,
`plans/markhand-web/backlog/phase-1b/issues/README.md:192-204`) indexes and
queries chunk text through PostgreSQL `to_tsvector('simple', …)` /
`plainto_tsquery('simple', …)`:

- Index side: trigger `chunks_set_tsv()` builds `NEW.tsv` from
  `markhand_accent_fold(heading_path || ' ' || body)`
  (`crates/server/migrations/0016_expand_chunks_accent_fold_tsv.sql:21-34`).
- Query side: `fts_search` accent-folds the query with
  `fileconv_core::intelligence::normalize_search_text` before
  `plainto_tsquery('simple', $N)` (`crates/server/src/db/search.rs:199-213`,
  `:238`, `:271`; `normalize_search_text` at `crates/core/src/intelligence.rs:488-490`
  is `accent_fold`, i.e. NFD-strip + `đ→d` + lowercase — no segmentation).
- `markhand_accent_fold` itself is the same NFD-strip/`đ→d`/lowercase transform,
  mirrored in SQL (`crates/server/migrations/0016_expand_chunks_accent_fold_tsv.sql:6-19`).

Index and query are therefore **symmetrically accent-folded** — that gap (raw
`simple` tsv vs. accent-folded query) is already closed by migration `0016`.
What remains missing is **word segmentation**: `simple` tokenizes on
whitespace/punctuation only, so a multi-syllable Vietnamese term such as
"khách hàng" or "Nghị định" is stored and matched as two independent
single-syllable tokens (`khach`, `hang` / `nghi`, `dinh`), not as one lexical
unit. This does not break `plainto_tsquery` (it ANDs the syllable tokens), but
it cannot distinguish "khách hàng" (customer) from an unrelated document that
merely contains both syllables "khách" and "hàng" elsewhere, and it cannot
weight the compound as a single stronger match the way a segmented lexical
index would.

In practice this is only the lexical leg of hybrid retrieval. The dense leg
(Qdrant vector search, `crates/server/src/services/retrieval/vector.rs`) and
rank fusion (`reciprocal_rank_fusion` / `RRF_K = 60.0`,
`crates/knowledge/src/rank.rs:7,39-45`, combined in `hybrid_rerank_score` at
`:64-76` and applied in `merge_rerank_hydrated`,
`crates/server/src/services/retrieval/mod.rs:669-730`) do not depend on
syllable segmentation, so recall for multi-syllable terms is mitigated by the
dense arm even when the lexical arm under- or over-matches.

## Decision

This ADR records the trade-off and a recommendation; it does not mandate an
implementation. **Recommendation: defer segmentation for Phase 1B**, with
explicit tracking, on the reasoning that (a) dense + RRF fusion already
covers recall for compound terms in current golden-quality evaluation, and
(b) a correct Vietnamese segmenter is nontrivial to add symmetrically without
either a new runtime dependency class or a costly backfill, both discussed
below. The maintainer owns the final call.

### Options considered

1. **Defer — track as a later-phase improvement (recommended default).**
   Keep `simple` + `markhand_accent_fold` as-is. No code change. Revisit when
   golden-quality evaluation shows the lexical arm's syllable-only matching
   is the binding constraint (as opposed to embedding quality or rerank
   weights).

2. **PostgreSQL dictionary / custom `TEXT SEARCH CONFIGURATION`.**
   Build a Vietnamese `ts_config` with a compound-aware dictionary
   (synonym/thesaurus-style multi-word entries) layered on the existing
   `simple` parser, or adopt a third-party Vietnamese FTS dictionary for
   PostgreSQL.
   - Pro: stays inside PostgreSQL; no new runtime process; index/query
     symmetry is enforced by using one `ts_config` in both the trigger and
     `fts_search`.
   - Con: dictionary-based compound recognition is a fixed-vocabulary
     enumeration, not general segmentation — new compounds ("Nghị định" style
     legal/domain terms) need dictionary maintenance; still requires a
     `tsv` backfill (`UPDATE chunks SET body = body;` pattern already used in
     migration `0016:37-38`) for every existing row when the config changes.

3. **Rust dictionary/longest-match segmenter run symmetrically at index and
   query time**, producing pre-segmented text (e.g. joining recognized
   compounds with an underscore) before it reaches `to_tsvector('simple', …)`
   / `plainto_tsquery('simple', …)`.
   - Pro: fits the project's "code do dự án làm chủ hoàn toàn" (own the code,
     no vendored runtime) constraint and its declared avoidance of a
     Python-runtime dependency (`CLAUDE.md`); a Rust longest-match segmenter
     against a Vietnamese compound-word list is a pure library call, easy to
     keep symmetric because the same function normalizes both the trigger
     input and the query input (mirroring how `markhand_accent_fold` in SQL
     already mirrors `normalize_search_text` in Rust).
   - Con: segmentation must be **exactly** symmetric between index and query
     or matches silently drop to zero (the same risk migration `0016`'s
     changelog already called out for accent-folding); requires a `tsv`
     backfill on rollout; longest-match dictionaries have known failure modes
     on ambiguous segmentation and out-of-vocabulary domain terms.
   - This is the option most consistent with the codebase's existing
     native-Rust-first architecture (`conv/*.rs`, `viet_legacy.rs`, etc. all
     avoid external interpreter runtimes) and is the **recommended future
     direction** if/when segmentation is scheduled.

4. **External segmenter service** (e.g. a Python-based Vietnamese NLP
   segmenter called over the network or as a sidecar process).
   - Pro: highest segmentation accuracy of the options (access to trained
     statistical/ML segmenters).
   - Con: introduces exactly the Python-runtime dependency class the project
     avoids elsewhere (see native-library policy in `CLAUDE.md`); adds a
     network hop/availability dependency to the indexing and query hot paths;
     symmetry risk is the same as option 3 but harder to guarantee once
     segmentation lives in a separately deployed/versioned service.

### Why defer is the default recommendation

- Dense + RRF already provides a recall safety net for compound terms today;
  there is no evaluation gate currently failing because of syllable-only
  lexical matching.
- Every non-defer option requires a full `tsv` backfill
  (`UPDATE chunks SET body = body;`-style rewrite) and — for options 2-4 —
  new index/query symmetry surface that must be tested as carefully as the
  accent-fold symmetry migration `0016` already tests.
- Should the maintainer decide to schedule this, option 3 (Rust dictionary
  segmenter, symmetric at index+query) is recommended over 2 and 4 because it
  keeps the dependency profile native-Rust, matching every other text
  pipeline in this repository.

## Consequences

- No behavior change from this ADR by itself (docs only).
- If deferred: multi-syllable lexical precision stays syllable-level; tracked
  as a known limitation, re-evaluated via `bench/markhand_web/reports/`
  golden-quality retrieval metrics rather than by inspection.
- If scheduled later under option 3: requires (a) a symmetric segmenter used
  by both the `chunks_set_tsv()` trigger path (or an equivalent Rust
  pre-processing step before insert) and `fts_search`'s query-side folding,
  (b) a full `tsv` backfill migration, and (c) golden-quality re-evaluation
  before/after to confirm recall/precision improved and did not regress
  accent-fold parity.

## Alternatives considered

See "Options considered" above; options 2 and 4 are not rejected outright,
they are documented so a future maintainer decision has the trade-offs on
record rather than needing to be rediscovered.

## Verification

No code changes accompany this ADR. If/when segmentation is implemented,
verification should extend the existing accent-fold parity pattern:

```bash
cargo test -p fileconv-server db::search
cargo test -p fileconv-knowledge --lib rank::
python3 bench/markhand_web/scripts/run_retrieval_eval.py
```

and add a backfill-parity check analogous to migration `0016`'s
`UPDATE chunks SET body = body;` re-trigger step.

## Exception lifecycle

N/A — this ADR proposes a deferral, not an exception to an existing control.
If the maintainer instead schedules segmentation now, this ADR should be
updated (or superseded) with an implementation decision and a follow-up
verification section before merge.
