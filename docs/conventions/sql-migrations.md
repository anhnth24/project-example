# SQL, data và migration conventions

Áp dụng cho PostgreSQL metadata/auth/ACL/jobs/quota/audit của Markhand Web. Phase F
chỉ khóa contract; Phase 0 quyết định RLS/partition strategy và Phase 1B tạo business
schema.

## Data model rules

- Primary key dùng UUID (`uuid`), timestamp dùng `timestamptz` UTC. Không lưu timezone
  local hoặc ID tuần tự làm public identifier.
- Bảng business tenant-scoped phải có `org_id uuid NOT NULL` và FK tới organization;
  index/query multi-column bắt đầu bằng `org_id` khi access path tenant-scoped.
- FK, `NOT NULL`, `CHECK`, unique constraint và enum/check choice phải diễn tả invariant
  tại database, không chỉ application code.
- Soft delete chỉ dùng khi retention/audit yêu cầu; read path vẫn phải filter/revoke
  ngay. Không dùng `deleted_at` để né ACL.
- Tên: snake_case; table plural; FK `<entity>_id`; index `idx_<table>__<columns>`;
  constraint `ck_`, `uq_`, `fk_`.

## Tenant and repository contract

- Repository business operation nhận `OrgContext`, không nhận `org_id` rời rạc từ HTTP
  input. Missing/empty scope fail closed.
- Query đọc/ghi tenant data luôn predicate `org_id = $org_id`, kể cả collection,
  document, chunk, job, audit và outbox. Candidate vector phải hydrate/authorize lại
  từ PostgreSQL trước khi trả content.
- RLS là decision Phase 0. Nếu enabled, transaction đặt context local; repository test
  phải bao gồm pool-leak và cross-org denial.

```sql
-- Ví dụ shape repository query; không phải business migration.
SELECT id, title, state
FROM documents
WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL;
```

## Transaction, lock and idempotency

- Transaction bao trùm invariant liên bảng; không giữ transaction khi gọi network,
  converter hoặc LLM.
- Dùng row lock/advisory lock có scope và timeout cho last-owner, quota, lease, sequence
  và concurrent state transition. Ghi rõ lock order để tránh deadlock.
- Mutation retryable có `idempotency_key`, unique constraint và response/replay policy.
  Retry không được tạo chunk, artifact, event hoặc quota debit trùng.

## Immutable migrations

Migrations ở `crates/server/migrations/` dùng tên:

```text
NNNN_<expand|backfill|cutover|contract>_<subject>.sql
```

- File đã manifest/merge không sửa, rename hay reorder. Thay đổi tiếp theo là migration
  mới.
- Header migration nêu phase, owner, expand/backfill/cutover/contract, lock/data-risk
  và rollback compatibility.
- `scripts/check-migration-manifest.py --check` xác thực filename/order/checksum.
  Sau review migration mới, chạy `--write-manifest` trong PR để pin checksum.

## Rollout

1. **Expand:** additive table/column/index nullable, compatibility trước.
2. **Backfill:** resumable/checkpointed, bounded batch, metrics và retry.
3. **Cutover:** application đọc/ghi dual path khi cần; evidence mixed-version.
4. **Contract:** bỏ old path chỉ sau retention/rollback window.

Application rollback không yêu cầu database rollback. Revert migration chỉ dành cho
schema chưa deployed; production rollback là forward-compatible migration mới.

## Required evidence

- Empty database apply; supported-version upgrade; checksum immutability.
- Explain/index evidence cho tenant query lớn; migration duration/lock budget.
- Cross-org denial, RLS/pool leakage (nếu ADR bật RLS), concurrent/idempotency test.
- Rollback compatibility and recovery test when persisted data changes.
