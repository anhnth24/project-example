# Definition of Ready và Definition of Done

## Definition of Ready

Issue chỉ chuyển sang `Ready` khi:

- outcome độc lập, acceptance criteria và scope rõ;
- dependency issue đã `Done`, external gate có evidence;
- owner và module boundary đã xác định;
- data/API/tenant impact được nêu; không áp dụng phải ghi `N/A`;
- test/evidence command hoặc fixture đã được xác định;
- security trigger đã được đánh giá.

## Definition of Done

Issue chỉ chuyển `Done` khi:

- [ ] Acceptance criteria đạt với evidence review được.
- [ ] Unit/integration/contract/denial tests liên quan xanh.
- [ ] Migration, rollback, performance hoặc manual evidence hoàn tất khi áp dụng.
- [ ] Không log document content, prompt, PII, token, key hoặc signed URL.
- [ ] Docs, OpenAPI/runbook/ADR được cập nhật khi contract thay đổi.
- [ ] Dependency/blocker và roadmap status được cập nhật.
- [ ] Không còn high/critical finding chưa disposition.

Merge PR không tự động là `Done` nếu issue cần deployment, gate hoặc evidence ngoài code.

## Security review trigger

Gắn review security bắt buộc khi thay đổi một trong các vùng: auth/session, org/RBAC/ACL,
upload/converter sandbox, object storage/signed URL, SQL/RLS/migration, secret/egress,
LLM content policy, audit/logging, dependency/native binary hoặc CI permission.
