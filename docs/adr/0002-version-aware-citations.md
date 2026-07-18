# ADR 0002: Version-aware retrieval and citations

- Status: Proposed
- Date: 2026-07-18
- Decision key: `document-artifact-model`
- Owners: `architecture-owner`, `retrieval-owner`
- Approver: Phase 0 architecture gate

## Context

A logical document changes over time. Returning only a document ID or mutable
“current” URL makes an old answer unverifiable and can silently mix facts from
different versions. Example: version 1 approves 10 million VND; the effective current
version approves 15 million VND.

## Decision

Document versions are immutable. A logical document has an atomic pointer to its
current **published and effective** version; the latest upload/draft is not
automatically current.

Retrieval exposes four explicit modes:

- `current` (default): search only current effective versions;
- `as_of`: resolve the latest published version effective at a timestamp;
- `compare`: retrieve at least two versions in one lineage and compute a cited delta;
- `history`: return the ordered version timeline with citations.

Every citation pins:

```text
org_id, logical_document_id, version_id, version_number,
content_sha256, chunk_id, page/slide/sheet, start, end, quote,
effective_at, is_current
```

An answer that mentions change must cite both old and new claims. The answer response
also carries `version_context`: current version, cited versions, effective dates,
history and a deterministic change note.

Example:

```text
Kinh phí hiện tại là 15 triệu đồng theo phiên bản 2 [CITE-0002].
Thay đổi: phiên bản 1 là 10 triệu đồng [CITE-0001], tăng 5 triệu đồng.
```

## Storage and indexing

- PostgreSQL is authoritative for logical documents, immutable versions, current
  pointer, effective interval and lineage.
- Chunk identity includes `version_id`; versions never share mutable chunks.
- Qdrant payload includes logical document/version identity, `is_current`,
  `effective_at` and index generation.
- Publishing a version updates the current pointer and index visibility atomically
  through outbox/jobs. Historical vectors may remain in a history generation but
  default search always filters current.

## Authorization

Resolving any historical citation performs fresh current authorization. Historical
ACL snapshots are provenance only. A caller needs explicit version-history permission;
revoked/deleted sources remain denied even when an old answer stored their IDs.

## Validation

- `current` answers may not cite superseded versions unless a version note explicitly
  compares history.
- `as_of` citations must be the effective version at the requested timestamp.
- `compare/history` citations must belong to one lineage and include at least two
  versions.
- Citation quotes are verified against the immutable version hash and UTF-8 span.
- Metrics include current-version accuracy, temporal accuracy, change accuracy and
  version-citation precision/recall.

## Consequences

- Citation payloads and UI badges become larger but remain auditable.
- Re-indexing and retention policies must distinguish current and historical vectors.
- P0-02 adds versioned gold; P0-06 fixes temporal retrieval/signature rules; P1B
  implements schema/services/API; Phase 2 renders version notes; Phase 3 adds full
  history/diff workflows.
