# P0-05 embedding evaluation (quality track)

- Generated: `2026-07-18T17:01:40.124989+00:00`
- Git commit: `c0a801ab68da1a08aeb4c46b50941d0849beb71d`
- Environment role: `reduced-smoke-cpu`
- Device: `cpu`
- Chunking: `heading-chunks-2000-v1`
- Runs per model: `3`
- Fixture manifest: `cf413fafabb136fe…`

## Quality vs gates

| Model | Family | Dims | Recall@5 | Hit@5 | MRR | nDCG@10 | Recall gate | Gap to best nDCG |
|---|---|---:|---:|---:|---:|---:|---|---|
| `AITeamVN/Vietnamese_Embedding` | bge-m3-vietnamese-ft | 1024 | 0.9174 | 0.9412 | 0.8130 | 0.8017 | PASS | PASS (0.0000) |
| `bkai-foundation-models/vietnamese-bi-encoder` | phobert-bi-encoder | 768 | 0.7997 | 0.8151 | 0.6553 | 0.6746 | FAIL | FAIL (0.1271) |

## Capacity note

- This track is CPU/GPU-auto quality only.
- VRAM/saturation/queue-depth evidence remains blocked on target NVIDIA GPU.

## Category breakdown (best candidate mean)

| Category | N | Recall@5 | Hit@5 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| abbreviation | 25 | 0.9200 | 0.9200 | 0.7682 | 0.7471 |
| conflict_acl_denied | 2 | 0.5000 | 0.5000 | 0.2679 | 0.3155 |
| conflict_as_of | 2 | 0.7500 | 1.0000 | 1.0000 | 0.8756 |
| conflict_current | 2 | 0.7500 | 1.0000 | 0.7500 | 0.6186 |
| conflict_history | 2 | 0.7500 | 1.0000 | 1.0000 | 0.8098 |
| diacritic_variant | 75 | 1.0000 | 1.0000 | 0.8804 | 0.8329 |
| long_context | 25 | 1.0000 | 1.0000 | 0.9533 | 0.9734 |
| multi_doc | 20 | 0.7417 | 0.9500 | 0.7808 | 0.7519 |
| named_entity | 50 | 0.9200 | 0.9200 | 0.8064 | 0.8205 |
| numeric_fact | 19 | 0.8421 | 0.8421 | 0.6610 | 0.6890 |
| table_numeric | 6 | 0.6667 | 0.6667 | 0.7113 | 0.7294 |
| temporal_as_of | 3 | 1.0000 | 1.0000 | 0.5833 | 0.6886 |
| temporal_current | 3 | 0.6667 | 0.6667 | 0.2893 | 0.4083 |
| version_compare | 3 | 1.0000 | 1.0000 | 0.8333 | 0.8569 |
| version_history | 1 | 1.0000 | 1.0000 | 0.5000 | 0.6934 |

## Immutable config snapshot

```json
{
  "chunkingVersion": "heading-chunks-2000-v1",
  "normalize": "l2",
  "ranking": "max-pool-chunk-cosine -> document",
  "models": [
    {
      "id": "aiteamvn-vietnamese-embedding",
      "hubId": "AITeamVN/Vietnamese_Embedding",
      "revision": "dea33aa1ab339f38d66ae0a40e6c40e0a9249568",
      "dimensions": 1024,
      "wordSegment": false
    },
    {
      "id": "bkai-vietnamese-bi-encoder",
      "hubId": "bkai-foundation-models/vietnamese-bi-encoder",
      "revision": "84f9d9ada0d1a3c37557398b9ae9fcedcdf40be0",
      "dimensions": 768,
      "wordSegment": true
    }
  ]
}
```

## Verdict

- Quality gate satisfied by at least one model: **YES**
- Selected draft (quality-only): `AITeamVN/Vietnamese_Embedding`
- P0-05 fully closed: **NO**

- Quality track executed on reduced-smoke hardware (CPU unless CUDA present).
- `AITeamVN/Vietnamese_Embedding` clears Recall@5 (0.9174 >= 0.85) with stable 3/3 runs.
- `bkai-foundation-models/vietnamese-bi-encoder` misses the gate on this corpus (0.7997); keep as negative control, not selectable.
- Next quality comparator should be another family that can pass (e.g. `BAAI/bge-m3` hybrid or `intfloat/multilingual-e5-large`).
- Capacity evidence (VRAM, saturation, queue depth, target GPU fingerprint) still required.
- ADR remains Proposed until capacity + approver sign-off.
- Restricted corpus must not leave to cloud providers; local/self-host only.
