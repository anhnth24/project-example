# Architecture Decision Records

ADR ghi quyết định khó đảo ngược hoặc ảnh hưởng nhiều boundary: dependency direction,
tenant/RLS, storage/vector topology, auth/session, public API compatibility, runtime
native, security/egress và migration strategy.

## Quy trình

1. Tạo `NNNN-slug.md` từ [`TEMPLATE.md`](TEMPLATE.md): Context, Decision,
   Consequences, Alternatives, Verification, Owners/Approver.
2. Giữ trạng thái `Proposed` cho tới khi owner/approver review evidence.
3. Đổi sang `Accepted`, `Rejected`, hoặc `Superseded by ADR NNNN`; không sửa lại
   history để làm mất lý do cũ.
4. PR thay đổi public contract hoặc security boundary phải link ADR; exception cần
   expiry và regression test.

## Index

| ADR | Status | Decision |
|---|---|---|
| [0001](0001-web-boundaries.md) | Accepted | Dependency boundaries của Markhand Web |
