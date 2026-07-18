# OpenAI dense rejection pack

Regenerated through the same harness as local P0-05 quality runs:

```bash
python3 bench/markhand_web/scripts/run_embedding_eval.py \
  --catalog bench/markhand_web/embedding/openai-models.yaml
```

Uses `FILECONV_EMBEDDING_API_KEY` against `https://api.openai.com/v1/embeddings`.
Synthetic/de-identified golden corpus only.

## Protocol parity with local harness

- chunking: `heading-chunks-2000-v1`
- payload: desktop `{heading}\n{text}`
- normalize: L2
- ranking: max-pool chunk cosine → document
- fixtures validated against `manifest.lock.json`
- per-query `rows` + `rankingSha256` in each `*/run-1.json`

## Dense Recall@5 (desktop-parity re-run)

Threshold observations only (non-gating track — not formal PASS/FAIL):

| Model | Dims | Recall@5 | Meets ≥0.85 |
|---|---:|---:|---|
| `text-embedding-3-large` | 3072 | 0.7010 | no |
| `text-embedding-3-large` | 1536 | 0.6982 | no |
| `text-embedding-3-small` | 1536 | 0.7710 | no |
| `text-embedding-ada-002` | 1536 | **0.7752** | no |

No OpenAI catalog model meets the Recall@5 ≥ 0.85 threshold. This track is
`openai-cloud-reject` (non-gating / not a selection draft).

See also `summary.json` and `bench/markhand_web/reports/openai-embedding-rejection.md`.
