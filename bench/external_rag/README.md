# External Vietnamese RAG pilot

This non-gating track runs Markhand's real conversion and retrieval pipeline over
50 public Vietnamese government documents. It exists to complement the small
synthetic Phase 0 golden corpus, not to replace its deterministic regression
coverage.

## Scope and provenance

- Source: the official Government document portal at
  `vanban.chinhphu.vn`.
- The committed `sources.lock.json` records detail URLs, attachment URLs,
  SHA-256 hashes, content types, and upstream validators.
- Downloaded originals, converted Markdown, and detailed result rows live in
  the gitignored `bench/corpus_external/`.
- Raw files are not redistributed. Review source terms before moving this
  benchmark into CI or publishing any derived corpus.
- Queries are derived from separately published official detail metadata. They
  exercise real converted documents and multi-chunk indexing, but retain
  lexical overlap and are therefore explicitly **non-gating**.

## Run

The pilot uses the pinned `AITeamVN/Vietnamese_Embedding` revision from
`bench/markhand_web/embedding/models.yaml`.

```bash
cargo build --release -p fileconv-cli
python3 bench/external_rag/scripts/run_pilot.py --self-test
python3 bench/external_rag/scripts/run_pilot.py
```

To intentionally refresh the source snapshot:

```bash
python3 bench/external_rag/scripts/run_pilot.py --refresh-sources
```

Refreshing changes the committed lock and requires reviewing all source,
license, hash, and result drift. For a small smoke run after refresh:

```bash
python3 bench/external_rag/scripts/run_pilot.py --limit 2
```

## Metrics and limits

The report separates lexical, pinned neural-vector, and frozen hybrid legs. It
records conversion success, empty-output rate, chunk count, Recall@5/10, Hit@5,
MRR, and nDCG@10.

The first pilot intentionally does **not** claim production retrieval quality:

- 50 documents remain a small candidate pool;
- relevance is document-level, not manually adjudicated page/chunk evidence;
- queries are official-metadata-derived rather than organic user questions;
- the source mix is government/legal-document heavy;
- no-answer, citation grounding, and answer faithfulness are not scored.

The next quality track should preserve this locked corpus while adding blind
human queries, difficult near-duplicate distractors, page/chunk qrels, and
no-answer cases.
