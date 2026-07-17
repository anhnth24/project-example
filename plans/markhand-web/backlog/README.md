# Markhand Web — milestone và issue backlog

Phase F là prerequisite cho backlog bốn bước:

0. Dựng engineering foundation: rules, skeleton, dev environment và CI.

1. Phase 0 + 1A được break thành issue có thể bắt đầu.
2. Phase 1B được tổ chức thành epic/dependency graph.
3. Issue implementation 1B chỉ chuyển `Blocked` → `Ready` khi gate Phase 0/1A
   tương ứng đã đạt.
4. Phase 1C–4 được chuẩn bị ở trạng thái `Backlog`; start/completion gate được tách
   rõ để UI mock có thể chạy song song mà không bỏ qua security gate.

## Milestones

Phase registry, phase-plan links, issue-catalog links và issue counts chỉ được khai báo
tại [`../README.md`](../README.md). Roadmap generator dùng registry đó làm nguồn sự thật,
tránh duy trì một bảng milestone trùng lặp tại đây.

## Trạng thái

- `Ready`: đủ thông tin và dependency để bắt đầu.
- `Blocked`: đã định nghĩa nhưng gate/dependency chưa đạt.
- `Backlog`: chưa activate milestone.
- `In progress`: đã có owner/branch và đang triển khai.
- `Review`: implementation và evidence đã sẵn sàng review.
- `Done`: acceptance criteria, required tests và gate evidence đều đạt.

Không chuyển `Done` chỉ vì code đã merge.

## Nhãn đề xuất

- Milestone: `web-foundation`, `web-p0`, `web-p1a`, `web-p1b`, `web-p1c`, `web-p2`, `web-p3`,
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
Phase F ─┬─> Phase 0 ─┐
         └─> Phase 1A ┴─> Phase 1B ─> Phase 1C ────────> Phase 2 complete
                                  └─> stable OpenAPI ─> Phase 2 UI/mock
                                                Phase 2 complete ─> Phase 3 ─> Phase 4
```

Phase F phải pass trước Phase 0/1A. Sau đó hai phase này chạy song song.
Phase 2 có **start gate**: UI/mock bắt đầu khi OpenAPI 1B ổn định. Integration trên
backend thật và **completion gate** vẫn phụ thuộc 1C-12/1C-13. Phase 0 và 1A có thể
chạy song song sau foundation gate.

## Quy tắc triển khai

- Mỗi issue tương ứng một logical PR nếu không có lý do kỹ thuật để gộp.
- Migration và code sử dụng migration có thể tách PR theo expand/cutover/contract.
- Security denial test nên viết exploit fixture trước hoặc cùng implementation.
- Benchmark PR phải commit harness/config/report schema; raw data lớn lưu artifact.
- Không đưa credential, model binary, corpus nhạy cảm hoặc benchmark hostname vào Git.
- Mọi PR server phải giữ desktop CI xanh.

## Đồng bộ lên GitHub Issues

Script: [`../../scripts/sync-github-issues.py`](../../scripts/sync-github-issues.py)

Mỗi catalog issue được map thành GitHub issue với:

- **Title:** `<MÃ> — <tiêu đề>` (ví dụ `F-01 — Architecture boundaries và dependency rules`)
- **Milestone:** theo phase (`Phase F`, `Phase 0`, `Phase 1A`, …)
- **Labels:** `markhand-web`, `docs`, `web-p0`/`web-p1b`/…, và trạng thái (`ready`/`blocked`/`backlog`)
- **Milestone progress:** status Markdown `Done` → GitHub issue `closed`; GitHub tự tăng
  closed count/progress của milestone. Sync không tự reopen issue đã đóng.

```bash
# xem trước 113 issue
python3 scripts/sync-github-issues.py --dry-run

# tạo milestone trước, rồi issue
python3 scripts/sync-github-issues.py --milestones-only
python3 scripts/sync-github-issues.py --create --update --sync-status

# hoặc export script rồi chạy trên máy có quyền (tạo cả milestone + issue)
python3 scripts/sync-github-issues.py --export-shell plans/markhand-web/backlog/create-github-issues.sh
bash plans/markhand-web/backlog/create-github-issues.sh
```

Workflow **Sync Markhand Web issues** cũng chạy tự động khi catalog backlog thay đổi
trên `master`; workflow tạo/update issue và đóng các issue đã `Done`.
