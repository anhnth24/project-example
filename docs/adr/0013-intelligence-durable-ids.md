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
   `idScheme` beside `packId`.
5. **Desktop knowledge** stores `intelligence_id_scheme` in SQLite `index_meta`.
   Missing/empty/mismatched scheme (or embedding signature mismatch) clears
   documents/chunks/FTS/meta and rebuilds HNSW via the existing atomic path.
   Legacy and current IDs must not coexist in one store.
6. **ADR 0006** server `IndexSignature` / knowledge `chunk_identity` remain
   unchanged and out of scope.

Dependency: workspace-pinned `sha2 = "=0.11.0"` (same crate family as ADR 0006).

## Consequences

- Positive: durable local IDs across toolchains; desktop refuses mixed ID eras.
- Positive: handoff consumers can gate on `schemaVersion` + `idScheme`.
- Negative: every pre-`sha256-v1` desktop index and handoff pack ID is obsolete.
- Migration: open/index with current plan → atomic SQLite/FTS wipe + HNSW clear
  + re-embed. Rollback = restore a pre-upgrade backup of
  `.markhand/knowledge.sqlite` and the HNSW partition directory, or delete both
  and rebuild under the desired scheme.
- Out of scope: server Postgres/Qdrant generations (ADR 0011); ADR 0006 digests.

## Alternatives considered

- Keep 64-bit SipHash-1-3 (`sip13-v1`): rejected for durable persisted IDs;
  SHA-256 matches ADR 0006 precedent and avoids short-hash collision risk.
- Soft-migrate by keeping old chunk rows beside new ones: rejected; citations
  and FTS would mix incompatible ID eras.
- Fold intelligence scheme into ADR 0006 `IndexSignature` fields: rejected for
  this change; server identity fixtures must stay frozen.

## Verification

```bash
cargo test -p fileconv-core --lib intelligence
cargo test -p fileconv-knowledge --features desktop-sqlite,desktop-hnsw --lib
cargo fmt --all -- --check
cargo metadata --locked --format-version 1 --no-deps
python3 scripts/check-dependency-policy.py
```

Required cases:

- pinned domain vectors for chunk/table/handoff-document/handoff-pack;
- visible IDs contain `sha256-v1`;
- handoff `schemaVersion == 2` and `idScheme == sha256-v1`;
- legacy SQLite fixture has empty `id_scheme` and upgrades only via atomic rebuild;
- missing `intelligence_id_scheme` with matching embedding signature still rebuilds.

## Exception lifecycle

N/A. Desktop caches rebuild in place (single-user). No mixed-scheme exception.
