# Phase 3 issues — Document Intelligence

Parent plan: [`../../../phase-3-intelligence.md`](../../../phase-3-intelligence.md)

<!-- roadmap-default-status: backlog -->

Mọi issue ở **Backlog**, blocked bởi Phase 2 gate trừ khi ghi khác.

## Dependency

```text
P3-01 → P3-02 → P3-03 ─┬→ P3-04 → P3-05
                        ├→ P3-06
                        ├→ P3-07
                        ├→ P3-08
                        ├→ P3-09
                        └→ P3-10
P3-01 + policy/quota → P3-11
P3-04..10 → P3-12
P3-04..11 → P3-13
P3-03..13 → P3-14
```

## P3-01 — Reusable intelligence service boundary

- **Plan/files:** Service nhận OrgContext, immutable versions, artifact store, LLM
  policy, jobs/audit; core deterministic algorithms không copy; desktop/web adapters.
- **Depends:** Phase 2 + existing jobs/storage/audit. **Acceptance/tests:** Không route
  gọi core trực tiếp; every call scoped; parity/adapter/missing-org/desktop tests.
- **Security:** No document logs. **Out:** watch folder/rewrite core algorithms.

## P3-02 — Versioned derived-artifact schema

- **Plan/files:** Artifact type/status/schema/hash/signature/creator/job/model/template;
  source-version joins, citations, versions/supersede/revision, source ACL/policy
  snapshot chỉ để provenance; opaque MinIO.
- **Depends:** P3-01 + ACL model. **Acceptance/tests:** Full provenance; retry no
  duplicate; fresh/upgrade/tenancy/reconcile/idempotency/conflict tests.
- **Security/migration:** Expand/backfill/cutover, no object key; ACL snapshot tuyệt
  đối không dùng để authorize. **Out:** mutable/shared artifact.

## P3-03 — Current-source ACL mỗi artifact access

- **Plan/files:** Resolve current intersection ACL/state on list/read/preview/download/
  export/citation; cache by ACL/membership version; invalidation only cleanup.
- **Depends:** P3-02 + 1C denial suite. **Acceptance/tests:** Revoke bất kỳ source deny
  ngay; cross-scope/revoke-race/cache/signed-url/existence tests.
- **Security:** Timeout fail closed; snapshot không authorize. **Out:** public links.

## P3-04 — Deterministic BRD/PRD job

- **Plan/files:** Authorized corpus → core handoff → validate IDs/citations/traceability/
  assumptions → checkpoint/persist artifacts.
- **Depends:** P3-01…03. **Acceptance/tests:** Đủ 10 artifacts; factual requirement có
  citation; không evidence→question; offline/empty/kill/retry/NFC tests.
- **Security:** Versioned payload/schema, content-safe audit. **Out:** third-party publish.

## P3-05 — Handoff edit/validate/ZIP export

- **Plan/files:** List/read/save/validate/export APIs; optimistic revision; streaming ZIP
  + hashes; reauthorize start/delivery.
- **Depends:** P3-03/04. **Acceptance/tests:** No overwrite; manifest round-trip/tamper/
  concurrent/revoke/large export tests.
- **Security:** Safe archive names, no public URL. **Out:** direct Jira/Confluence import.

## P3-06 — Quality và immutable reprocess

- **Plan/files:** Persist quality/recommendations; new native/OCR/VLM jobs/version;
  before/after/provenance; quota/sandbox/egress.
- **Depends:** P3-01…03 + converter. **Acceptance/tests:** Source immutable; policy deny
  no cloud; OCR/VLM/fallback/quota/cancel/retry tests.
- **Security:** VLM deny-by-default. **Out:** fake unsupported success.

## P3-07 — Citation-backed summarization

- **Plan/files:** Document/collection/corpus summary; audience/length/language;
  extractive fallback; LLM authorized passages only.
- **Depends:** P3-01…03 + retrieval. **Acceptance/tests:** Every fact attributable;
  fallback/insufficient/timeout/revoke/factuality golden tests.
- **Security:** Provider metadata only, no prompt logs. **Out:** web research.

## P3-08 — PII detection/reviewed redaction

- **Plan/files:** Extensible rules, `pii.manage`, sensitive reports, review before
  derived version/export, full audit.
- **Depends:** P3-02/03 + RBAC. **Acceptance/tests:** Original hash unchanged; findings
  authorized; precision/recall/completeness/overlap/Unicode/permission tests.
- **Security:** No PII logs; prohibited data no GLM. **Out:** legal completeness guarantee.

## P3-09 — Schema/table edit và safe CSV

- **Plan/files:** Stable table/cell IDs, source revision, patch surrounding-safe,
  optimistic conflict, formula neutralization.
- **Depends:** P3-02/03. **Acceptance/tests:** Byte-preserving round trip; escaped pipes/
  multiline/stable ID/formula/ACL tests.
- **Security:** Source hash binding. **Out:** spreadsheet formulas/merged edit/realtime.

## P3-10 — Versions, diff và three-way merge

- **Plan/files:** List/read/diff, base/parent provenance, merge conflicts, expected
  revision saves.
- **Depends:** P3-02/03. **Acceptance/tests:** Unrelated merge, exact conflicts, no
  stale overwrite; Unicode/CRLF/concurrent/revoked-base tests.
- **Security:** Immutable rows/opaque keys. **Out:** collaborative editing.

## P3-11 — Prompt/model/quota/cloud policy

- **Plan/files:** Separate system/untrusted text; no tool/scope change; classification
  and token caps; version templates; timeout/cancel/circuit/fallback.
- **Depends:** P3-01 + quota/classification. **Acceptance/tests:** Injection/egress deny/
  token race/provider failure/template rollback tests.
- **Security:** Secret manager, metadata-only audit. **Out:** general agents/tools.

## P3-12 — Intelligence web workspace

- **Plan/files:** Corpus/Handoff/Quality/Versions/Tables/Privacy/Export panels; typed
  state, permissions, browser downloads, deep links, cancel/clear on scope change.
- **Depends:** P3-04…10 APIs. **Acceptance/tests:** Independent panels; no stale PII/
  artifact state; component/a11y/Playwright flows.
- **Security:** No storage-key URL. **Out:** Q&A panel/watch/native dialogs.

## P3-13 — Task-specific golden evaluation

- **Plan/files:** Separate datasets/runners for citation, requirements/traceability,
  summary factuality, PII, redaction, table and merge; version all inputs.
- **Depends:** P3-04…11. **Acceptance/tests:** Independent numeric thresholds/regression
  budgets; reproducible case-level failures; retrieval score không thay task metrics.
- **Security:** Licensed/de-identified corpus. **Out:** blended “AI quality” score.

## P3-14 — Intelligence denial/audit/reconcile/release gate

- **Plan/files:** Extend denial to artifacts/routes; object-row reconcile; audit
  coverage; realistic corpus/fallback E2E.
- **Depends:** P3-03…13. **Acceptance/tests:** Zero leakage; current ACL/revoke races;
  no canonical mutation; provider outage; desktop regression.
- **Security:** Reconcile không restore revoked visibility. **Out:** OIDC/production infra.

## Exit gate

Phase 3 cần current-source authorization, BRD/PRD citation validity, independent
quality thresholds, immutable originals, safe table/version/export, deterministic
fallback và complete audit/denial evidence.
