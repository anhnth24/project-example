# [ID] Tiêu đề kết quả cần đạt

## Metadata

- Milestone:
- Labels:
- Status: `Ready | Blocked | Backlog`
- Parent epic:
- Owner:
- Gate registry keys:

## Objective

Mô tả một kết quả có thể review/verify độc lập. Không mô tả bằng hoạt động chung
chung như “làm backend” hoặc “cải thiện security”.

## Context

- Hiện trạng và bằng chứng trong repo.
- Vì sao issue cần thiết.
- Contract/invariant phải giữ.

## Implementation plan

1. Các bước theo thứ tự dependency.
2. Data/API/state transitions cần thêm hoặc thay đổi.
3. Error/fallback/cancellation/idempotency behavior.
4. Observability/audit cần emit.
5. Documentation/runbook cần cập nhật.

## Files/modules

Liệt kê đường dẫn từ repository root. Với file chưa tồn tại, đánh dấu `(new)`.

- `path/to/existing.rs`
- `path/to/new.rs` (new)

## Dependencies / blocks

- Depends on:
- External decision/hardware:
- Blocks:
- Điều kiện chuyển `Blocked` → `Ready`:

Diagram milestone chỉ là critical path; phần này là dependency authority.

## Acceptance criteria

- [ ] Outcome quan sát được, không chỉ “code đã merge”.
- [ ] Backward compatibility/desktop invariant đạt.
- [ ] Error và degraded mode đạt.
- [ ] Numeric gate trỏ tới gate registry.
- [ ] Docs/runbook/API contract cập nhật.

## Required tests / evidence

- [ ] Unit tests:
- [ ] Integration/contract tests:
- [ ] Denial/security tests:
- [ ] Migration/upgrade/rollback tests:
- [ ] Performance/evaluation:
- [ ] Manual/deployed evidence:

Ghi command, fixture/workload, environment và vị trí artifact/report.

## Security and migration notes

- Tenant/ACL behavior:
- Sensitive data/logging:
- Secret/egress behavior:
- Migration strategy:
- Rollback/compatibility:

Nếu không áp dụng, ghi rõ `N/A` và lý do; không xóa section.

## Out of scope

Liệt kê rõ các hạng mục dễ bị hiểu nhầm là thuộc issue này.

## Definition of done

- [ ] Acceptance checklist hoàn tất.
- [ ] Required tests/evidence hoàn tất.
- [ ] Dependency/blocker đã cập nhật.
- [ ] Không còn high/critical finding chưa disposition.
- [ ] PR đã merge nhưng chỉ đóng issue sau khi deployed/gate evidence đạt nếu issue
      yêu cầu deployment.
