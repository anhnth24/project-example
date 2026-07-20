# P0-05 embedding evaluation

Quality-track harness for Vietnamese dense retrieval on the Phase 0 golden corpus.

**Selected for Markhand Web POC/1B:** `AITeamVN/Vietnamese_Embedding`
(`runtime_path=local-neural`, ADR 0005). GLM cloud is not the server embedding
path; see `docs/journals/2026-07-20-aiteamvn-local-embedding-decision.md`.

## Candidates

Pinned in [`models.yaml`](models.yaml):

1. **best:** `AITeamVN/Vietnamese_Embedding` (1024-d, BGE-M3 fine-tune)
2. **min:** `bkai-foundation-models/vietnamese-bi-encoder` (768-d, PhoBERT; needs `pyvi` segmentation)

Payload format matches desktop SQLite indexing: `{heading}\n{text}` (no markdown `#` prefix).

## Run

```bash
python3 -m pip install --user -r bench/markhand_web/requirements-embedding.txt
python3 bench/markhand_web/scripts/run_embedding_eval.py --self-test
python3 bench/markhand_web/scripts/run_embedding_eval.py --runs 3
```

Each `--runs` iteration **reloads** the model (independent loads). Gate statistics
come from `models.yaml` (authoritative):

- Recall@5 → **min** across runs
- best-model nDCG gap → **max** across per-run gaps

Gating protocol requires ≥2 model families and ≥3 runs (otherwise pass
`--allow-nongating` and the comparative verdict is disabled). Fixtures are
validated against `manifest.lock.json`.

Outputs:

- `bench/markhand_web/embedding/results/<model-id>/run-*.json` (includes per-query `rows` + `rankingSha256`)
- `bench/markhand_web/reports/embedding-evaluation.md`
- `bench/markhand_web/embedding/results/summary.json`

OpenAI cloud rejection (same harness, `FILECONV_EMBEDDING_API_KEY`):

```bash
python3 bench/markhand_web/scripts/run_embedding_eval.py \
  --catalog bench/markhand_web/embedding/openai-models.yaml
```

Outputs under `embedding/results/openai-rejected/` (per-model `run-*.json` with
rows + ranking fingerprints).

CPU is enough for local quality. Capacity/VRAM evidence still requires a NVIDIA
GPU environment (`on-prem-reference`) and is out of scope for this smoke track.
