# External Vietnamese RAG pilot

This non-gating track runs Markhand's production Rust conversion and desktop
knowledge pipeline over 50 public Vietnamese government documents. It exists to
complement the small synthetic Phase 0 golden corpus, not to replace its
deterministic regression coverage.

## Scope and provenance

- Source: the official Government document portal at
  `vanban.chinhphu.vn`.
- The committed `sources.lock.json` records detail URLs, attachment URLs,
  SHA-256 hashes, content types, and upstream validators.
- Downloaded originals, converted Markdown, and detailed result rows live in
  the gitignored `bench/corpus_external/`.
- Raw files are not redistributed. Review source terms before moving this
  benchmark into CI or publishing any derived corpus.
- Python only downloads sources, verifies hashes, invokes the runner, and renders
  the report. The Rust runner uses `Converter`, production `build_corpus`,
  SQLite FTS/vector storage, index signatures, and
  `desktop::service::hybrid_search`.
- Queries are derived from separately published official detail metadata. They
  exercise real converted documents and multi-chunk indexing, but retain lexical
  overlap and are therefore explicitly **non-gating**.

## Run

The pilot uses the pinned `AITeamVN/Vietnamese_Embedding` revision through the
same OpenAI-compatible local service used by Markhand workers.

```bash
make dev-up
cargo build --release -p fileconv-knowledge \
  --features external-rag-pilot --example external_rag_pilot
python3 bench/external_rag/scripts/run_pilot.py --self-test
python3 bench/external_rag/scripts/run_pilot.py
make dev-down
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

The report records conversion success, empty-output rate, production chunk
count, and Recall@5/10, Hit@5, MRR, and nDCG@10 from the actual hybrid service.
The detailed gitignored JSON preserves production chunk hits, scores, anchors,
warnings, and conversion failures for diagnosis.

The first pilot intentionally does **not** claim production retrieval quality:

- 50 documents remain a small candidate pool;
- relevance is document-level, not manually adjudicated page/chunk evidence;
- queries are official-metadata-derived rather than organic user questions;
- the source mix is government/legal-document heavy;
- no-answer, citation grounding, and answer faithfulness are not scored.

The next quality track should preserve this locked corpus while adding blind
human queries, difficult near-duplicate distractors, page/chunk qrels, and
no-answer cases.
