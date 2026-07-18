# P0-06 chunk size comparison

Pinned chunking version: `heading-chunks-2000-v1` (`MAX_CHARS=2000`).

## Candidates

| max_chars | non-empty chunks | mean chars | max chars | citation spans covered |
|---:|---:|---:|---:|---|
| 1000 | 29 | 170.4 | 266 | 305/305 |
| **2000** | **29** | **170.4** | **266** | **305/305** |
| 4000 | 29 | 170.4 | 266 | 305/305 |

Notes:

- On this golden corpus all three candidates produce the same heading-section
  chunk set (no section exceeds 1000 characters after heading splits).
- Hybrid neural Recall@5 (AITeamVN, frozen RRF `vectorWeight=0.55`) on the
  pinned 2000 catalog is reported in `retrieval/summary.json`.
- Pinning `heading-chunks-2000-v1` preserves desktop/core parity for longer
  real-world sections even though this synthetic corpus does not stress the cap.

Decision: keep `heading-chunks-2000-v1` (desktop/core parity + no unnecessary
fragmentation on this corpus).
