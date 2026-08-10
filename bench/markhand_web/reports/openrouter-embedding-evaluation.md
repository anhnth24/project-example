# P0-05 embedding evaluation (quality track)

- Generated: `2026-08-10T17:09:08.895080+00:00`
- Track: `openrouter-adr0016`
- Git commit: `0271ae51d828213ab5f5673e8a7a57aac27a52fe`
- Dirty worktree: `False`
- Dirty paths: `(none)`
- Gating protocol: `YES`
- Environment role: `reduced-smoke-cpu`
- Device: `openai-api`
- Chunking: `heading-chunks-2000-v1`
- Payload format: `{heading}\n{text}`
- Runs per model: `3` (independent loads)
- Gate stats: Recall@5=`min`, best-model nDCG gap=`max`
- Fixture manifest: `f191c5a3f893d987…`
- Fixture files checked: `32`

## Quality vs thresholds

| Model | Family | Dims | Recall@5 (min) | Hit@5 | MRR | nDCG@10 (min) | Recall≥0.85 | Gap≤0.02 |
|---|---|---:|---:|---:|---:|---:|---|---|
| `qwen/qwen3-embedding-8b` | qwen3-embedding-8b | 4096 | 0.9436 | 0.9538 | 0.7737 | 0.8072 | PASS | PASS (0.0000) |
| `qwen/qwen3-embedding-8b` | qwen3-embedding-8b-matryoshka | 1024 | 0.9181 | 0.9286 | 0.7577 | 0.7942 | PASS | PASS (0.0152) |

## Capacity note

- This track is CPU/GPU-auto quality only.
- VRAM/saturation/queue-depth evidence remains blocked on target NVIDIA GPU.

## Category breakdown (selected draft, last run)

| Category | N | Recall@5 | Hit@5 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| abbreviation | 25 | 1.0000 | 1.0000 | 0.7533 | 0.7649 |
| conflict_acl_denied | 2 | 0.5000 | 0.5000 | 0.2679 | 0.3155 |
| conflict_as_of | 2 | 0.7500 | 1.0000 | 1.0000 | 0.8756 |
| conflict_current | 2 | 0.7500 | 1.0000 | 0.7500 | 0.6533 |
| conflict_history | 2 | 0.8750 | 1.0000 | 1.0000 | 0.9810 |
| diacritic_variant | 75 | 0.9333 | 0.9333 | 0.7224 | 0.7709 |
| long_context | 25 | 1.0000 | 1.0000 | 0.9067 | 0.9484 |
| multi_doc | 20 | 0.9417 | 1.0000 | 0.9750 | 0.9588 |
| named_entity | 50 | 0.9200 | 0.9200 | 0.7722 | 0.8116 |
| numeric_fact | 19 | 1.0000 | 1.0000 | 0.7105 | 0.7571 |
| table_numeric | 6 | 0.8333 | 0.8333 | 0.6528 | 0.6474 |
| temporal_as_of | 3 | 1.0000 | 1.0000 | 0.4167 | 0.6000 |
| temporal_current | 3 | 1.0000 | 1.0000 | 0.8333 | 0.8976 |
| version_compare | 3 | 1.0000 | 1.0000 | 1.0000 | 0.9465 |
| version_history | 1 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |

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
      "id": "qwen3-embedding-8b-full",
      "hubId": "qwen/qwen3-embedding-8b",
      "provider": "openai-compatible",
      "revision": "openai-alias-observed-2026-08-10",
      "revisionRequested": "openai-alias-observed-2026-08-10",
      "modelMutability": "mutable-alias",
      "observedAt": "2026-08-10T17:08:30.134618+00:00",
      "dimensions": 4096,
      "maxSeqLength": 32768,
      "batchSize": 64,
      "device": "openai-api",
      "wordSegment": false,
      "wordSegmenter": null,
      "normalize": "l2"
    },
    {
      "id": "qwen3-embedding-8b-d1024",
      "hubId": "qwen/qwen3-embedding-8b",
      "provider": "openai-compatible",
      "revision": "openai-alias-observed-2026-08-10",
      "revisionRequested": "openai-alias-observed-2026-08-10",
      "modelMutability": "mutable-alias",
      "observedAt": "2026-08-10T17:09:03.624079+00:00",
      "dimensions": 1024,
      "maxSeqLength": 32768,
      "batchSize": 64,
      "device": "openai-api",
      "wordSegment": false,
      "wordSegmenter": null,
      "normalize": "l2"
    }
  ]
}
```

## Ranking fingerprints

- `qwen/qwen3-embedding-8b`: 4957c52b9c85ca5fc5ce65a251479d28137a7577b71fa7e20fa5f950631ca6e9, 72077279e409548e5738aac0ba803adb16c357943f601e3642d698cf6ea7c331, ca5d7eb35c6164cd8d92a603b620e9bdb17f232fdf0fde956c376e09673d2df5
- `qwen/qwen3-embedding-8b`: 97f95c9b1794af1ae0572fdd3bfb1d4aafaf5acbd78660912c6e2d7072432d82, a16a0f2e575fc013ba83db41169a819b8420efb867c0bceeebf4e47fd7a219aa, f9996799b067828b7797cf66fd937fbb591029a1a42882955c54c5dfe9900e94

## Verdict

- Gating protocol (≥2 families / ≥3 runs): **YES**
- Both quality gates satisfied by selected draft: **YES**
- Selected draft (quality-only): `qwen/qwen3-embedding-8b`
- P0-05 fully closed: **NO**

- Quality track executed with independent model loads per run.
- Gate thresholds/statistics loaded from catalog YAML.
- Selection requires both Recall@5 and best-model-gap gates under gating protocol.
- Per-query rankings retained in run-*.json with rankingSha256 fingerprints.
- Golden markdown/queries validated against manifest.lock.json.
- Capacity evidence (VRAM, saturation, queue depth, target GPU) still required.
- ADR remains Proposed until capacity + approver sign-off.
- Restricted corpus must not leave to cloud providers; local/self-host only.
