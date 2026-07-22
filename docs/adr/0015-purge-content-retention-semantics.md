# ADR 0015: Immutable-content retention semantics for document purge (P1B-I07)

- Status: Proposed
- Date: 2026-07-22
- Decision key: `purge-content-retention-semantics`
- Owners: retrieval-owner, security-owner, storage-owner
- Approver: Phase 1B architecture gate
- Related issues/PRs: P1B-I07 (PR #245); `docs/markhand-web-upload-policy.md`

## Context

`crates/server/src/services/deletion.rs` implements the tombstone-then-purge
lifecycle. `request_delete` moves a document `Indexed → Tombstoned` and sets
`deleted_at` (`:73-160`). `purge_document` then runs a checkpointed pipeline
(`:162-243`):

1. `delete_qdrant_points` — delete all Qdrant points scoped to the document
   across every recorded index signature (`:280-296`).
2. `delete_minio_objects_audited` → `delete_minio_object_batch` — delete
   **every** MinIO object key returned by
   `document_versions::list_object_keys_by_document` (`:390-426`).
3. `delete_chunks` — `DELETE FROM chunks WHERE … document_id = $2`
   (`crates/server/src/db/chunks.rs:181-193`, called from `deletion.rs:472-484`).
4. `finalize_purged` — transitions `Tombstoned → Purged`
   (`crates/server/src/services/deletion.rs:515-522`).

`list_object_keys_by_document` (`crates/server/src/db/document_versions.rs:246-273`)
unions three key sources: `document_versions.original_object_key` (the
immutable original upload blob), `document_versions.markdown_object_key`, and
`derived_artifacts.object_key`. Its own doc comment says *"purge deletes only
the objects they name"* (`:245`) — i.e. it is descriptive of what purge does,
not a retention promise. Nothing in `purge_document` special-cases
`original_object_key` to keep it; **every** content blob named by these rows,
including the original upload, is deleted from MinIO.

Meanwhile, purge does **not** `DELETE` the PG lineage rows themselves:
`document_versions` and `derived_artifacts` rows are left in place (only
`chunks` rows and Qdrant points are removed), and the document row moves to
`Purged` rather than being deleted. So immutable "versions/artifacts are
retained" currently holds only for **PostgreSQL metadata** (version numbers,
object-key history, hashes, filenames) — not for the **content blobs** those
rows point to. After purge, `original_object_key` and `markdown_object_key`
are dangling references: the row survives, the object does not.

The P1B-I07 issue text
(`plans/markhand-web/backlog/phase-1b/issues/README.md:179-188`) does not
explicitly specify content-blob retention. Its plan line says "idempotent
vector/object cleanup" and its scope line says **"Out: legal hold/full ACL
revoke"** (`:188`), which is evidence the issue was scoped to *not* include a
legal-hold retention path — but it stops short of stating whether the
originals themselves must survive purge.

`docs/markhand-web-upload-policy.md` §3 (quarantine lifecycle) says: *"Keep
quarantined originals for 14 days by default, unless legal hold extends
retention"* (`:61-62`). This sentence describes a **different lifecycle
stage**: it is about objects sitting in **pre-index quarantine** (before the
allowlist/malware/size checks promote or reject them), not about an
already-indexed, user-requested, tombstoned-then-purged document. The two are
easy to conflate by name ("keep originals … unless legal hold") but currently
govern different states in the document lifecycle, and the upload-policy
document does not cross-reference the purge path at all. This ADR treats that
as a **documentation gap**, not a code bug: nothing here contradicts current
behavior, but a reader could reasonably expect the "unless legal hold" carve-out
in §3 to also apply after purge, which it does not today (no legal-hold field
or path exists in `deletion.rs`, consistent with I07's "Out: … legal hold").

## Decision

This ADR records the retention trade-off and a recommendation; it does not
change any code. **Recommendation: keep purge as a terminal, hard-delete
operation for Phase 1B (current behavior = option 1 below)**, and update the
P1B-I07 issue text to state this explicitly, because it matches the issue's
own declared scope ("Out: legal hold/full ACL revoke") and because retention
without a designed lifecycle (option 2) is a larger, separate feature that
touches storage cost, access control, and compliance policy. The maintainer
owns the final call.

### Options considered

1. **Purge is terminal; content blobs are destroyed (current behavior).**
   `original_object_key`, `markdown_object_key`, and all `derived_artifacts`
   objects are deleted from MinIO; only PG lineage rows (which no longer
   resolve to any object) and the `Purged` document state remain.
   - Matches a GDPR-style "right to erasure" hard delete: once purged, the
     tenant's content is actually gone, not just hidden.
   - Simplest to reason about and already implemented/tested
     (`crates/server/src/services/deletion.rs` tests at `:722-760`).
   - Cannot be walked back: there is no "restore a purged document" path, by
     design (`request_delete` only accepts `Indexed`; `purge_document` refuses
     anything except `Tombstoned`/already-`Purged`, `:172-181`).

2. **Retain immutable originals even after purge (audit/legal hold).**
   Skip deleting `original_object_key` (and optionally `markdown_object_key`)
   during purge, moving it to a separate retention/cold-storage lifecycle
   instead of deleting it outright.
   - Supports audit and legal-hold requirements that need the original bytes
     after a user-initiated delete.
   - Requires a **new** retention lifecycle: a place to store "purged but
     retained" objects, a policy for how long, who can read them (tenant user
     access must not survive purge, but auditor/compliance access might), and
     a true final-erasure path for when retention actually expires — none of
     which exists today. This is not a small addition to `purge_document`; it
     is a new subsystem with its own access-control and storage-cost model.
   - Increases storage cost indefinitely for every purged document unless a
     hard retention-expiry job is also built.
   - Directly conflicts with a strict reading of "right to erasure" purge
     semantics unless the retention path is itself gated by a documented
     legal basis (e.g. explicit legal hold, not default behavior).

### Cross-check against `docs/markhand-web-upload-policy.md`

The upload-policy §3 "14 days … unless legal hold" clause applies to
**pre-index quarantine**, a state that (per the state machine implied by
`deletion.rs` and the issue list) a document has already left by the time it
can be tombstoned/purged. Recommending option 1 does not contradict §3: §3's
retention window governs objects that never made it to `Indexed`; purge
governs objects that did. If the maintainer instead chooses option 2, §3
should be revised to clarify that the "unless legal hold" carve-out is meant
to extend across the whole lifecycle (quarantine **and** post-purge), not
just quarantine — as currently worded it does not make that claim.

### Suggested P1B-I07 wording (proposed, not decided here)

If option 1 is confirmed, add to
`plans/markhand-web/backlog/phase-1b/issues/README.md` under P1B-I07, e.g.
appended to the **Security/migration** line:

> Purge is terminal: it deletes chunks, Qdrant points, and **all** MinIO
> objects named by `document_versions`/`derived_artifacts`, including the
> original upload (`original_object_key`). Only PG lineage rows and the
> `Purged` document state remain; they no longer resolve to any object.
> Legal hold / retained-original purge is explicitly out of scope (see ADR
> 0015).

If option 2 is chosen instead, the issue text (and `deletion.rs`, and this
ADR) would need to change together, and `docs/markhand-web-upload-policy.md`
§3 would need a cross-reference so the two retention windows (pre-index
quarantine vs. post-purge) are not read as contradictory.

## Consequences

- No behavior change from this ADR by itself (docs only).
- If option 1 is confirmed as-is: no code change; only the issue text and
  possibly this ADR's status move to `Accepted`.
- If option 2 is chosen: requires new code in `purge_document` (skip/relocate
  `original_object_key` deletion), a new retention-store/lifecycle,
  authorization rules for who can read retained-but-purged content, a
  hard-erasure job, and updates to `docs/markhand-web-upload-policy.md` and
  the P1B-I07 issue text together. That is materially larger than the current
  purge implementation and should be its own issue, not a silent change to
  `deletion.rs`.

## Alternatives considered

See "Options considered" above. A middle option (retain only
`markdown_object_key`/derived artifacts but always delete
`original_object_key`) was not analyzed in depth here because the P1B-I07
"Out: legal hold" scope line and the GDPR-style erasure goal both point at
originals being the sensitive artifact to actually erase; if the maintainer
wants that middle ground it should be spelled out as its own option before
implementation.

## Verification

No code changes accompany this ADR. If the maintainer confirms option 1,
verification is the existing test suite plus the issue-text update:

```bash
cargo test -p fileconv-server services::deletion
```

If option 2 is chosen, verification must add: an authorization test that a
purged document's retained original is not tenant-readable through any
existing document/version API, and a hard-erasure-after-retention test.

## Exception lifecycle

N/A — this ADR documents current behavior and proposes confirming it, not an
exception to a control. If option 2 (retain-after-purge) is chosen instead,
it should be scoped as a narrow, explicitly-approved legal-hold exception
path with its own owner and expiry, not a default for all purges.
