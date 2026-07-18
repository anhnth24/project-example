# ADR 0003: Cross-document conflict warnings and resolution history

- Status: Proposed
- Date: 2026-07-18
- Decision key: `cross-document-conflict-model`
- Owners: `architecture-owner`, `retrieval-owner`, `product-owner`
- Approver: Phase 0 architecture gate

## Context

BA requirements, design specifications and implementation notes can assert incompatible
facts. A difference is not automatically a conflict: claims must overlap in subject,
scope, effective time and semantics. Conflicts may also disappear in later versions,
but historical answers must still explain what was inconsistent and how it was fixed.

## Decision

The system extracts normalized claims with:

```text
claim_key, subject, predicate, typed value/unit, scope,
logical_document_id, version_id, effective interval, citation
```

Deterministic rules detect high-confidence numeric, enum, date/limit and
MUST-versus-MUST-NOT contradictions. Optional LLM classification may suggest candidates
but cannot create a warning without cited claims and validator approval.

A conflict is immutable evidence plus mutable lifecycle:

```text
open → resolved
  ├→ accepted_exception
  └→ false_positive
```

Each conflict records severity, both claim citations, first-detected version/time,
current status, resolution versions, resolution note and audit metadata. Updating a
document never deletes conflict history.

Default UI behavior is a warning, not a publish block. A separately configured policy
may block explicitly enumerated high-severity claim types.

## Version behavior

- Current mode evaluates only current effective versions and shows unresolved warnings.
- As-of mode evaluates claims effective at the requested time.
- History mode shows conflicts that were open then resolved.
- A resolved conflict cites both old conflicting versions and current aligned versions.

Example:

```text
v1 BA: kinh phí 10 triệu [CITE-0001]
v1 design: kinh phí 15 triệu [CITE-0002] → warning, difference 5 triệu

v2 BA: kinh phí 15 triệu [CITE-0003]
v2 design: kinh phí 15 triệu [CITE-0004] → resolved
```

## Authorization

Every warning and historical citation is authorized at read time. Conflict records must
not reveal the existence, title, value or version of a source the caller cannot access.
When only one side is authorized, the API returns no cross-source details.

## Validation and metrics

- claim extraction precision/recall;
- conflict precision/recall by deterministic type;
- unresolved-current warning accuracy;
- resolved-history accuracy;
- evidence citation precision/recall;
- false-positive and accepted-exception rate.

## Consequences

- P0-02 carries open/resolved conflict gold.
- P0-06 defines claim keys, deterministic rules and evaluation thresholds.
- P1B adds claim/conflict tables, incremental detection and APIs.
- Phase 2 renders warning badges, side-by-side evidence and resolution history.
- Phase 3 provides richer diff/merge and conflict triage workflows.
