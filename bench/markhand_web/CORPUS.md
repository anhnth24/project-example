# Phase 0 golden corpus

The generated corpus contains:

- 29 synthetic Vietnamese document versions covering all 10 ingest format families;
- 260 retrieval queries with UTF-8 byte spans and graded judgments;
- multi-document answers plus current/as-of/compare/history version citations;
- a 50-query stratified review packet;
- 10 adversarial upload fixtures;
- empty-transcript audio tones for hallucination/conversion checks.

Regenerate and validate:

```bash
python3 -m pip install --user -r bench/markhand_web/requirements-corpus.txt
python3 bench/markhand_web/scripts/generate_corpus.py
python3 scripts/validate_corpus.py --reproducible
```

The generator requires the exact package and DejaVu font fingerprints in
`generator-environment.lock.json`. Query adjudication is content-bound through the
`review-sample.tsv` SHA-256; changing the sample invalidates approval.

Chunk catalog for `heading-chunks-2000-v1` is pinned in
`retrieval/expected-chunks.tsv` (P0-06). Query and conflict citation `chunkId`
fields are filled from that catalog via
`bench/markhand_web/scripts/fill_citation_chunk_ids.py` (mechanical span→chunk
annotation; adjudication keeps `sampleSemanticSha256` for the chunkId-null
packet).
