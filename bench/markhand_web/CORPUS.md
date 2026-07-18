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
python3 bench/markhand_web/scripts/generate_expected_chunks.py
python3 bench/markhand_web/scripts/fill_citation_chunk_ids.py
python3 scripts/validate_corpus.py --reproducible
```

`fill_citation_chunk_ids.py` rewrites `manifest.lock.json` after annotation so
in-place regeneration stays checksum-valid. Canonical reproducibility
(`validate_corpus.py --reproducible`) runs the same pipeline in a temp tree and
compares `manifest.lock.json`.

The generator requires the exact package and DejaVu font fingerprints in
`generator-environment.lock.json`. Query adjudication is content-bound through
`sampleSemanticSha256` (chunkId-null review packet); mechanical `chunkId` fill may
change `sampleSha256` without re-adjudication.

Chunk catalog for `heading-chunks-2000-v1` is pinned in
`retrieval/expected-chunks.tsv` (P0-06). Query and conflict citation `chunkId`
fields are filled from that catalog via
`bench/markhand_web/scripts/fill_citation_chunk_ids.py`.
