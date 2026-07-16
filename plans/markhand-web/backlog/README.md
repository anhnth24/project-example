# Markhand Web — milestone và issue backlog

Backlog này triển khai tuần tự bốn bước:

1. Phase 0 + 1A được break thành issue có thể bắt đầu.
2. Phase 1B được tổ chức thành epic/dependency graph.
3. Issue implementation 1B chỉ chuyển `Blocked` → `Ready` khi gate Phase 0/1A
   tương ứng đã đạt.
4. Phase 1C–4 được chuẩn bị ở trạng thái `Backlog`; start/completion gate được tách
   rõ để UI mock có thể chạy song song mà không bỏ qua security gate.

## Milestones

| Milestone | Catalog | Issue | Trạng thái ban đầu | Exit gate |
|---|---|---:|---|---|
| Phase 0 | [`phase-0/issues/README.md`](phase-0/issues/README.md) | 10 | Ready/Blocked theo hardware | Decision gates |
| Phase 1A | [`phase-1a/issues/README.md`](phase-1a/issues/README.md) | 10 | Ready theo dependency | Desktop parity |
| Phase 1B | [`phase-1b/issues/README.md`](phase-1b/issues/README.md) | 24 | Blocked | Single-org POC |
| Phase 1C | [`phase-1c/issues/README.md`](phase-1c/issues/README.md) | 13 | Backlog | Multi-org denial |
| Phase 2 | [`phase-2/issues/README.md`](phase-2/issues/README.md) | 16 | Backlog | Web E2E |
| Phase 3 | [`phase-3/issues/README.md`](phase-3/issues/README.md) | 14 | Backlog | Intelligence quality/security |
| Phase 4 | [`phase-4/issues/README.md`](phase-4/issues/README.md) | 14 | Backlog | Production go-live |
| **Tổng** | | **101** | | |

## Trạng thái

- `Ready`: đủ thông tin và dependency để bắt đầu.
- `Blocked`: đã định nghĩa nhưng gate/dependency chưa đạt.
- `Backlog`: chưa activate milestone.
- `In progress`: đã có owner/branch và đang triển khai.
- `Review`: implementation và evidence đã sẵn sàng review.
- `Done`: acceptance criteria, required tests và gate evidence đều đạt.

Không chuyển `Done` chỉ vì code đã merge.

## Nhãn đề xuất

- Milestone: `web-p0`, `web-p1a`, `web-p1b`, `web-p1c`, `web-p2`, `web-p3`,
  `web-p4`.
- Type: `epic`, `feature`, `security`, `benchmark`, `infra`, `migration`,
  `test`, `docs`.
- Area: `core`, `knowledge`, `server`, `worker`, `storage`, `auth`, `web`,
  `intelligence`, `ops`.
- State helper: `blocked`, `needs-decision`, `needs-hardware`.

## Quy tắc issue

Catalog dùng card rút gọn nhưng phải chứa thông tin tương đương. Khi tạo issue trên
tracker, dùng [`ISSUE_TEMPLATE.md`](ISSUE_TEMPLATE.md) và mở rộng thành ít nhất tám
section bắt buộc:

1. Objective.
2. Implementation plan.
3. Files/modules.
4. Dependencies/blocks.
5. Acceptance criteria.
6. Required tests/evidence.
7. Security/migration notes.
8. Out of scope.

Nếu card gộp `Acceptance/tests`, `Security/migration` hoặc dùng tiêu đề làm objective,
người tạo issue phải tách lại theo template; trường không áp dụng ghi rõ
`N/A — không thay đổi persisted contract/schema`. Đường dẫn rút gọn trong card được
hiểu tương đối với namespace đầu tiên đã nêu; tracker issue phải dùng đường dẫn từ
repo root.

Issue chỉ được activate khi:

- dependency issue đã `Done`;
- quyết định/gate liên quan có evidence;
- numeric threshold lấy từ gate registry, không tự đặt trong implementation PR;
- không có thay đổi scope ẩn.

Các diagram trong catalog là **critical path rút gọn**. Trường `Dependencies/blocks`
của từng issue là nguồn sự thật đầy đủ khi tạo tracker dependency.

## Dependency cấp milestone

```text
Phase 0 ───────────┐
                   ├─> Phase 1B ─> Phase 1C ────────> Phase 2 complete
Phase 1A ──────────┘         └─> stable OpenAPI ─> Phase 2 UI/mock
                                                Phase 2 complete ─> Phase 3 ─> Phase 4
```

Phase 2 có **start gate**: UI/mock bắt đầu khi OpenAPI 1B ổn định. Integration trên
backend thật và **completion gate** vẫn phụ thuộc 1C-12/1C-13. Phase 0 và 1A có thể
chạy song song.

## Quy tắc triển khai

- Mỗi issue tương ứng một logical PR nếu không có lý do kỹ thuật để gộp.
- Migration và code sử dụng migration có thể tách PR theo expand/cutover/contract.
- Security denial test nên viết exploit fixture trước hoặc cùng implementation.
- Benchmark PR phải commit harness/config/report schema; raw data lớn lưu artifact.
- Không đưa credential, model binary, corpus nhạy cảm hoặc benchmark hostname vào Git.
- Mọi PR server phải giữ desktop CI xanh.
