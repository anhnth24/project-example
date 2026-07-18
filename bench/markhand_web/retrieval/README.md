# P0-06 retrieval track

## Artifacts

- `expected-chunks.tsv` — canonical chunk catalog for `heading-chunks-2000-v1`
- `expected-chunks.meta.json` — counts + content hash
- `summary.json` — latest scaffold metrics

## Regenerate

```bash
python3 bench/markhand_web/scripts/generate_expected_chunks.py
python3 bench/markhand_web/scripts/generate_expected_chunks.py --check
python3 bench/markhand_web/scripts/run_retrieval_eval.py --self-test
python3 bench/markhand_web/scripts/run_retrieval_eval.py
```

Identity digests match `crates/knowledge` schema v2 (see ADR 0006).

## Status

- Chunk catalog + citation span resolve: done
- Local-hash lexical/vector/hybrid scaffold: done (frozen RRF weights)
- Fill `chunkId` into golden citations: open
- Neural hybrid + RRF tune + temporal/conflict gates: open
