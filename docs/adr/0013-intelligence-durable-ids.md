# ADR 0013: Durable intelligence IDs (`sha256-v1`) and desktop rebuild

- Status: Accepted
- Date: 2026-07-21
- Decision key: `intelligence-durable-ids`
- Owners: retrieval-owner, desktop-owner
- Approver: CORE-T9 review gate
- Related issues/PRs: CORE-T9; ADR 0006; ADR 0011
- Supersedes: interim `sip13-v1` experiment on the CORE-T9 branch (never a release contract)

## Context

`fileconv-core` intelligence assigned corpus chunk, markdown table, and handoff
fingerprint IDs with `std::collections::hash_map::DefaultHasher`. That hasher is
not a cross-Rust-version contract, so persisted desktop SQLite/FTS/HNSW rows and
handoff packs could drift across builds. ADR 0006 already pins **server**
document/chunk/index identity to length-delimited SHA-256; desktop local
intelligence IDs still needed an explicit durable scheme without changing the
ADR 0006 digest field layout.

SQLite metadata and the on-disk HNSW cache are separate stores: they cannot share
one filesystem transaction. Any migration story that claims cross-store atomicity
is false; safety must come from scheme validation and exact-search fallback.

## Decision

1. **Scheme** `sha256-v1` (`INTELLIGENCE_ID_SCHEME` in `fileconv-core`):
   SHA-256 over length-prefixed (`u64` BE) fields:
   `markhand-intelligence-id`, scheme, purpose domain, then payload bytes.
   Integers use fixed-width `u64` BE. **No** `std::hash::Hash` serialization.
2. **Purpose domains** (independent digests):
   - `chunk` — corpus chunk IDs
   - `table` — markdown table IDs
   - `handoff-document` — per-document content fingerprint
   - `handoff-pack` — pack fingerprint (slug + mode tag + document digests)
3. **Visible IDs** embed the scheme:
   `chunk-sha256-v1-{hex}`, `table-sha256-v1-{hex}`,
   `handoff-sha256-v1-{slug}-{nonce}-{hex}`.
4. **Handoff schema** bumps to `HANDOFF_SCHEMA_VERSION = 2` and persists
   `idScheme` beside `packId`. Desktop `load_persisted_pack` rejects any pack
   whose `schemaVersion` / `idScheme` are not exactly v2 / `sha256-v1`, with
   guidance to regenerate the Knowledge Pack. App TypeScript types mirror the
   same fields/constants.
5. **Desktop SQLite** stores `intelligence_id_scheme` in `index_meta`.
   Missing/empty/mismatched scheme (or embedding signature mismatch) wipes
   documents/chunks/FTS/meta in one SQLite transaction and re-indexes. Legacy
   and current chunk IDs must not coexist in one SQLite database.
6. **Desktop HNSW** persists `idScheme` in `manifest.json` and folds a non-empty
   scheme into the partition name. `is_available` / `search` / `rebuild` validate
   scheme equality. A stale ANN left behind when HNSW clear fails after a SQLite
   commit is therefore **not addressable** under the new scheme; hybrid search
   self-heals via exact cosine and warnings. Clear/rebuild failures never roll
   back a committed SQLite upgrade.
7. **ADR 0006** server `IndexSignature` / knowledge `chunk_identity` remain
   unchanged and out of scope.

Dependency: workspace-pinned `sha2 = "=0.11.0"` (same crate family as ADR 0006).

## Consequences

- Positive: durable local IDs across toolchains; desktop refuses mixed ID eras.
- Positive: handoff consumers can gate on `schemaVersion` + `idScheme`.
- Positive: SQLite/HNSW divergence cannot silently serve old ANN IDs after a
  scheme upgrade.
- Negative: every pre-`sha256-v1` desktop index and handoff pack ID is obsolete.
- Migration: open/index with current plan → SQLite/FTS wipe + best-effort HNSW
  clear/rebuild. If HNSW clear/rebuild fails, search continues with exact cosine
  until a later successful rebuild. Rollback = restore a pre-upgrade backup of
  `.markhand/knowledge.sqlite` **and** the HNSW partition directory together, or
  delete both and rebuild under the desired scheme.
- Out of scope: server Postgres/Qdrant generations (ADR 0011); ADR 0006 digests.

## Alternatives considered

- Keep 64-bit SipHash-1-3 (`sip13-v1`): rejected for durable persisted IDs;
  SHA-256 matches ADR 0006 precedent and avoids short-hash collision risk.
- Soft-migrate by keeping old chunk rows beside new ones: rejected; citations
  and FTS would mix incompatible ID eras.
- Pretend SQLite+HNSW clear is one atomic unit: rejected; not true on disk.
  Scheme-gated ANN + exact fallback is the safety property instead.
- Fold intelligence scheme into ADR 0006 `IndexSignature` fields: rejected for
  this change; server identity fixtures must stay frozen.

## Verification

```bash
cargo test -p fileconv-core --lib intelligence
cargo test -p fileconv-knowledge --features desktop-sqlite,desktop-hnsw --lib
cargo test -p fileconv-desktop --lib intelligence::
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
```

Required cases:

- pinned domain vectors for chunk/table/handoff-document/handoff-pack;
- visible IDs contain `sha256-v1`;
- handoff `schemaVersion == 2` and `idScheme == sha256-v1`;
- load rejects schema v1 and idScheme mismatch with regenerate guidance;
- legacy SQLite fixture has empty `id_scheme` and upgrades via SQLite rebuild;
- HNSW manifest/partition scheme mismatch is unavailable; clear failure after
  SQLite commit keeps SQLite and falls back to exact search.

## Exception lifecycle

N/A. Desktop caches rebuild in place (single-user). No mixed-scheme exception.
