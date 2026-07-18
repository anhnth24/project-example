# P0-05 embedding evaluation

Quality-track harness for Vietnamese dense retrieval on the Phase 0 golden corpus.

## Candidates

Pinned in [`models.yaml`](models.yaml):

1. **best:** `AITeamVN/Vietnamese_Embedding` (1024-d, BGE-M3 fine-tune)
2. **min:** `bkai-foundation-models/vietnamese-bi-encoder` (768-d, PhoBERT; needs `pyvi` segmentation)

## Run

```bash
python3 -m pip install --user -r bench/markhand_web/requirements-embedding.txt
python3 bench/markhand_web/scripts/run_embedding_eval.py --runs 3
```

Outputs:

- `bench/markhand_web/embedding/results/<model-id>/run-*.json`
- `bench/markhand_web/reports/embedding-evaluation.md`
- `bench/markhand_web/embedding/results/summary.json`

CPU is enough for quality. Capacity/VRAM evidence still requires a NVIDIA GPU
environment (`on-prem-reference`) and is out of scope for this smoke track.
