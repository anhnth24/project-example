# Phase 0 golden corpus

The generated corpus contains:

- 27 synthetic Vietnamese documents covering all 10 ingest format families;
- 250 retrieval queries with UTF-8 byte spans and graded judgments;
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

Expected chunk IDs are intentionally absent until P0-06 fixes chunking version.
