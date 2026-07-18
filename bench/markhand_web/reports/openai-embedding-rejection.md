# P0-05 OpenAI embedding rejection (non-gating)

- Generated: `2026-07-18T17:27:06.311228+00:00`
- Track: `openai-cloud-reject`
- Git commit: `0e6fac936c1fc3dcdb4fe42ee9bc3111c21a81e2`
- Dirty worktree: `False`
- Dirty paths: `(none)`
- Gating protocol: `NO`
- Environment role: `reduced-smoke-cpu`
- Device: `openai-api`
- Chunking: `heading-chunks-2000-v1`
- Payload format: `{heading}\n{text}`
- Runs per model: `1` (independent loads)
- Gate stats: Recall@5=`min`, best-model nDCG gap=`max`
- Fixture manifest: `cf413fafabb136fe…`
- Fixture files checked: `32`

## Quality vs thresholds

| Model | Family | Dims | Recall@5 (min) | Hit@5 | MRR | nDCG@10 (min) | Recall≥0.85 | Gap≤0.02 |
|---|---|---:|---:|---:|---:|---:|---|---|
| `text-embedding-3-large` | openai-3-large | 3072 | 0.7010 | 0.7143 | 0.6005 | 0.6255 | no | no (0.0398) |
| `text-embedding-3-large` | openai-3-large-matryoshka | 1536 | 0.6982 | 0.7143 | 0.5844 | 0.6148 | no | no (0.0505) |
| `text-embedding-3-small` | openai-3-small | 1536 | 0.7710 | 0.7815 | 0.6383 | 0.6619 | no | yes (0.0035) |
| `text-embedding-ada-002` | openai-ada-002 | 1536 | 0.7752 | 0.7899 | 0.6270 | 0.6653 | no | yes (0.0000) |

## Capacity note

- Cloud OpenAI `/v1/embeddings` reject track; local GPU capacity N/A.
- OpenAI model ids are mutable aliases; pins are observation dates only.

## Category breakdown (best observed model, last run)

| Category | N | Recall@5 | Hit@5 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| abbreviation | 25 | 0.9200 | 0.9200 | 0.6390 | 0.6629 |
| conflict_acl_denied | 2 | 0.5000 | 0.5000 | 0.5250 | 0.5000 |
| conflict_as_of | 2 | 0.7500 | 1.0000 | 0.7500 | 0.7625 |
| conflict_current | 2 | 1.0000 | 1.0000 | 1.0000 | 0.9197 |
| conflict_history | 2 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |
| diacritic_variant | 75 | 0.6267 | 0.6267 | 0.5208 | 0.5443 |
| long_context | 25 | 1.0000 | 1.0000 | 0.7800 | 0.8774 |
| multi_doc | 20 | 0.8500 | 1.0000 | 0.9750 | 0.9275 |
| named_entity | 50 | 0.8600 | 0.8600 | 0.6619 | 0.7091 |
| numeric_fact | 19 | 0.5263 | 0.5263 | 0.3468 | 0.4406 |
| table_numeric | 6 | 0.5000 | 0.5000 | 0.2824 | 0.3867 |
| temporal_as_of | 3 | 1.0000 | 1.0000 | 0.6667 | 0.7629 |
| temporal_current | 3 | 1.0000 | 1.0000 | 0.5833 | 0.6986 |
| version_compare | 3 | 1.0000 | 1.0000 | 0.8333 | 0.8569 |
| version_history | 1 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |

## Config snapshot (OpenAI aliases are mutable)

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
      "id": "text-embedding-3-large-full",
      "hubId": "text-embedding-3-large",
      "provider": "openai-compatible",
      "revision": "openai-alias-observed-2026-07-18",
      "revisionRequested": "openai-alias-observed-2026-07-18",
      "modelMutability": "mutable-alias",
      "observedAt": "2026-07-18T17:26:55.448723+00:00",
      "dimensions": 3072,
      "maxSeqLength": 8191,
      "batchSize": 64,
      "device": "openai-api",
      "wordSegment": false,
      "wordSegmenter": null,
      "normalize": "l2"
    },
    {
      "id": "text-embedding-3-large-d1536",
      "hubId": "text-embedding-3-large",
      "provider": "openai-compatible",
      "revision": "openai-alias-observed-2026-07-18",
      "revisionRequested": "openai-alias-observed-2026-07-18",
      "modelMutability": "mutable-alias",
      "observedAt": "2026-07-18T17:26:58.557028+00:00",
      "dimensions": 1536,
      "maxSeqLength": 8191,
      "batchSize": 64,
      "device": "openai-api",
      "wordSegment": false,
      "wordSegmenter": null,
      "normalize": "l2"
    },
    {
      "id": "text-embedding-3-small-full",
      "hubId": "text-embedding-3-small",
      "provider": "openai-compatible",
      "revision": "openai-alias-observed-2026-07-18",
      "revisionRequested": "openai-alias-observed-2026-07-18",
      "modelMutability": "mutable-alias",
      "observedAt": "2026-07-18T17:27:01.215256+00:00",
      "dimensions": 1536,
      "maxSeqLength": 8191,
      "batchSize": 64,
      "device": "openai-api",
      "wordSegment": false,
      "wordSegmenter": null,
      "normalize": "l2"
    },
    {
      "id": "text-embedding-ada-002",
      "hubId": "text-embedding-ada-002",
      "provider": "openai-compatible",
      "revision": "openai-alias-observed-2026-07-18",
      "revisionRequested": "openai-alias-observed-2026-07-18",
      "modelMutability": "mutable-alias",
      "observedAt": "2026-07-18T17:27:04.177755+00:00",
      "dimensions": 1536,
      "maxSeqLength": 8191,
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

- `text-embedding-3-large`: dd17dff4441e4ffb912a6d94147b684b49513312aa561fde8b827e4763561031
- `text-embedding-3-large`: 372b072578235449081fe223e8c98202e731672b319e4da54ba894b633490252
- `text-embedding-3-small`: 82b9c69134856f2c2bad2f6a72245d0034db5204c44ede90803b51acfeec6eb3
- `text-embedding-ada-002`: f0d1acbb0297dbe146858a54db46f9f63199e66b9aaa0bf920753d6879a8235f

## Verdict

- Gating protocol: **NO** (threshold observations only; no formal PASS/FAIL).
- Best observed model (non-draft): `text-embedding-ada-002`
- Selected draft: `null`
- P0-05 fully closed: **NO**

- OpenAI cloud reject track: same desktop payload/chunking/ranking as local harness; OpenAI model ids are mutable aliases (observation pin only).
- NON-GATING run: threshold observations only; no formal PASS/FAIL or selectedDraft.
- Quality track executed with independent model loads per run.
- Gate thresholds/statistics loaded from catalog YAML.
- Selection requires both Recall@5 and best-model-gap gates under gating protocol.
- Per-query rankings retained in run-*.json with rankingSha256 fingerprints.
- Golden markdown/queries validated against manifest.lock.json.
- Capacity evidence (VRAM, saturation, queue depth, target GPU) still required.
- ADR remains Proposed until capacity + approver sign-off.
- Restricted corpus must not leave to cloud providers; local/self-host only.
