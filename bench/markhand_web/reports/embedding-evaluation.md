# P0-05 embedding evaluation (quality track)

- Generated: `2026-07-18T17:29:28.584328+00:00`
- Track: `quality-cpu-smoke`
- Git commit: `cf14d8f52290110540618aa9059642b45f8bfad1`
- Dirty worktree: `False`
- Dirty paths: `(none)`
- Gating protocol: `YES`
- Environment role: `reduced-smoke-cpu`
- Device: `cpu`
- Chunking: `heading-chunks-2000-v1`
- Payload format: `{heading}\n{text}`
- Runs per model: `3` (independent loads)
- Gate stats: Recall@5=`min`, best-model nDCG gap=`max`
- Fixture manifest: `cf413fafabb136fe…`
- Fixture files checked: `32`

## Quality vs thresholds

| Model | Family | Dims | Recall@5 (min) | Hit@5 | MRR | nDCG@10 (min) | Recall≥0.85 | Gap≤0.02 |
|---|---|---:|---:|---:|---:|---:|---|---|
| `AITeamVN/Vietnamese_Embedding` | bge-m3-vietnamese-ft | 1024 | 0.9261 | 0.9538 | 0.8067 | 0.7992 | PASS | PASS (0.0000) |
| `bkai-foundation-models/vietnamese-bi-encoder` | phobert-bi-encoder | 768 | 0.7962 | 0.8151 | 0.6508 | 0.6700 | FAIL | FAIL (0.1292) |

## Capacity note

- This track is CPU/GPU-auto quality only.
- VRAM/saturation/queue-depth evidence remains blocked on target NVIDIA GPU.

## Category breakdown (selected draft, last run)

| Category | N | Recall@5 | Hit@5 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| abbreviation | 25 | 0.9600 | 0.9600 | 0.7242 | 0.7162 |
| conflict_acl_denied | 2 | 0.5000 | 0.5000 | 0.2672 | 0.3155 |
| conflict_as_of | 2 | 0.7500 | 1.0000 | 1.0000 | 0.8756 |
| conflict_current | 2 | 0.7500 | 1.0000 | 0.7500 | 0.6186 |
| conflict_history | 2 | 0.6250 | 1.0000 | 1.0000 | 0.8038 |
| diacritic_variant | 75 | 1.0000 | 1.0000 | 0.8733 | 0.8308 |
| long_context | 25 | 1.0000 | 1.0000 | 0.9533 | 0.9740 |
| multi_doc | 20 | 0.7083 | 0.9500 | 0.7802 | 0.7572 |
| named_entity | 50 | 0.9200 | 0.9200 | 0.8217 | 0.8302 |
| numeric_fact | 19 | 0.8947 | 0.8947 | 0.6053 | 0.6571 |
| table_numeric | 6 | 0.8333 | 0.8333 | 0.7460 | 0.7539 |
| temporal_as_of | 3 | 1.0000 | 1.0000 | 0.6667 | 0.7548 |
| temporal_current | 3 | 0.6667 | 0.6667 | 0.2893 | 0.4083 |
| version_compare | 3 | 1.0000 | 1.0000 | 0.8333 | 0.8569 |
| version_history | 1 | 1.0000 | 1.0000 | 0.5000 | 0.6934 |

## Immutable config snapshot

```json
{
  "chunkingVersion": "heading-chunks-2000-v1",
  "normalize": "l2",
  "ranking": "max-pool-chunk-cosine -> document",
  "payloadFormat": "{heading}\\n{text}",
  "gates": {
    "G0-RET-RECALL-AT-5": {
      "threshold": 0.85,
      "statistic": "min"
    },
    "G0-RET-BEST-MODEL-GAP": {
      "threshold": 0.02,
      "statistic": "max"
    }
  },
  "models": [
    {
      "id": "aiteamvn-vietnamese-embedding",
      "hubId": "AITeamVN/Vietnamese_Embedding",
      "provider": "sentence-transformers",
      "revision": "dea33aa1ab339f38d66ae0a40e6c40e0a9249568",
      "revisionRequested": "dea33aa1ab339f38d66ae0a40e6c40e0a9249568",
      "modelMutability": "immutable-sha",
      "observedAt": "2026-07-18T17:29:18.601624+00:00",
      "dimensions": 1024,
      "maxSeqLength": 2048,
      "batchSize": 16,
      "device": "cpu",
      "wordSegment": false,
      "wordSegmenter": null,
      "normalize": "l2"
    },
    {
      "id": "bkai-vietnamese-bi-encoder",
      "hubId": "bkai-foundation-models/vietnamese-bi-encoder",
      "provider": "sentence-transformers",
      "revision": "84f9d9ada0d1a3c37557398b9ae9fcedcdf40be0",
      "revisionRequested": "84f9d9ada0d1a3c37557398b9ae9fcedcdf40be0",
      "modelMutability": "immutable-sha",
      "observedAt": "2026-07-18T17:29:27.076235+00:00",
      "dimensions": 768,
      "maxSeqLength": 256,
      "batchSize": 32,
      "device": "cpu",
      "wordSegment": true,
      "wordSegmenter": "pyvi",
      "normalize": "l2"
    }
  ]
}
```

## Ranking fingerprints

- `AITeamVN/Vietnamese_Embedding`: 45d8e8de7cbd22abdbf22fc5bf7de1f66344740cf2227c8803059dd050e4f6a3
- `bkai-foundation-models/vietnamese-bi-encoder`: 0fbb87f9230cfcb18db88cb36f8468a50fd4e41adb3f2d718f1be4935f5c8b39

## Verdict

- Gating protocol (≥2 families / ≥3 runs): **YES**
- Both quality gates satisfied by selected draft: **YES**
- Selected draft (quality-only): `AITeamVN/Vietnamese_Embedding`
- P0-05 fully closed: **NO**

- Quality track executed with independent model loads per run.
- Gate thresholds/statistics loaded from catalog YAML.
- Selection requires both Recall@5 and best-model-gap gates under gating protocol.
- Per-query rankings retained in run-*.json with rankingSha256 fingerprints.
- Golden markdown/queries validated against manifest.lock.json.
- Capacity evidence (VRAM, saturation, queue depth, target GPU) still required.
- ADR remains Proposed until capacity + approver sign-off.
- Restricted corpus must not leave to cloud providers; local/self-host only.
