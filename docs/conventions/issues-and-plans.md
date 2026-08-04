# Chuẩn issue và implementation plan

Tài liệu này là nguồn chuẩn cho format issue và plan. Definition of Ready/Done và security
trigger nằm tại [`delivery.md`](delivery.md); quality gate nằm tại
[`../runbooks/contributor-setup.md`](../runbooks/contributor-setup.md).

## Nguồn dữ liệu authoritative

- Với Markhand Web, phase catalog trong
  `plans/markhand-web/backlog/<phase>/issues/README.md` là authority. GitHub issue được
  render/synchronize từ catalog bằng `scripts/sync-github-issues.py`.
- Với subsystem khác, dùng catalog/milestone hiện có của subsystem đó. Không đưa issue core,
  desktop hoặc CLI vào phase Web chỉ để lấy ID. Nếu chưa có catalog phù hợp, phải xác nhận
  tracking location/milestone với owner trước khi tạo remote issue.
- Một issue chỉ có một outcome có thể review độc lập. Không tạo issue mới nếu code hoặc issue
  hiện có đã đáp ứng outcome; khi đó phải thu hẹp thành gap hoặc enhancement thực sự.
- Không tự tạo GitHub issue, milestone, label hay thay đổi trạng thái remote nếu user chưa
  cho phép rõ ràng.

## Format issue

### Tiêu đề và trạng thái

Tiêu đề canonical:

```text
<ID> — <outcome ngắn gọn>
```

ID phải theo convention của catalog/milestone sở hữu. Trạng thái hợp lệ:

- `Backlog`: scope đã ghi nhưng chưa đủ điều kiện bắt đầu;
- `Blocked — <lý do chính xác>`: thiếu dependency, quyết định hoặc external evidence;
- `Ready`: vượt qua toàn bộ Definition of Ready;
- `In progress`: đang triển khai;
- `Review`: implementation/evidence đang được review;
- `Done`: vượt qua Definition of Done, không chỉ vì PR đã merge.

Issue mới không được tạo trực tiếp ở trạng thái `Done`.

### Catalog entry canonical

```markdown
## <ID> — <outcome ngắn gọn>

- **Status:** <Backlog | Blocked — exact blocker | Ready>
- **Objective:** <một outcome quan sát và review độc lập được>
- **Implementation plan:** <hướng kỹ thuật theo thứ tự; failure/degraded behavior>
- **Files/modules:** <owner; module boundary; file dự kiến>
- **Dependencies/blocks:** <issue ID/external gate; evidence cần để unblock>
- **Acceptance criteria:** <hành vi pass/fail quan sát được, gồm negative path>
- **Required tests/evidence:** <command, fixture, environment và artifact cụ thể>
- **Security/migration:** <data/API/tenant impact; security trigger; migration/rollback,
  hoặc N/A có lý do>
- **Out of scope:** <giới hạn rõ để tránh scope creep>
```

Các tên field trên là canonical vì `scripts/sync-github-issues.py` dùng chúng để tạo GitHub
body. Acceptance criteria mô tả hành vi cần chứng minh; implementation plan mô tả cách dự
kiến thực hiện. Không trộn hai loại nội dung.

Chỉ dùng `Ready` khi đã có:

1. một outcome và scope rõ;
2. acceptance criteria quan sát được;
3. owner và module boundary;
4. dependency/external gate đã hoàn tất với evidence;
5. data/API/tenant impact hoặc `N/A` có lý do;
6. test command/fixture/evidence được chỉ định;
7. security trigger được đánh giá;
8. out-of-scope rõ ràng.

Thiếu dữ kiện quan trọng phải dùng `Backlog`/`Blocked`; không tự suy đoán owner, approval,
dependency completion, benchmark hoặc security evidence.

### Roadmap consistency

Khi thêm issue vào Markhand Web:

1. thêm issue vào phase dependency graph và summary nếu áp dụng;
2. cập nhật issue count của phase và tổng `Issue-level backlog (... issues)` tại
   `plans/markhand-web/README.md`;
3. không thêm `Plan file` placeholder; chỉ liên kết sau khi file plan thật đã được tạo;
4. chạy:

   ```bash
   python3 scripts/build-roadmap.py
   python3 scripts/build-roadmap.py --check
   python3 scripts/sync-github-issues.py --dry-run
   ```

Chỉ khi được cho phép mới chạy `--create` hoặc `--update`.

## Format implementation plan

### Vị trí và tên file

Plan được lưu tại:

```text
plans/reports/plan-YYYY-MM-DD-<lowercase-id>-<slug>.md
```

- Issue đang mở: ngày trong filename là ngày plan được tạo; metadata dùng
  `Created: YYYY-MM-DD`.
- Hồ sơ lịch sử cho issue đã đóng: ngày trong filename và `Issue closed: YYYY-MM-DD` phải
  lấy từ verified GitHub close timestamp.
- Không dùng ngày chạy generator thay cho ngày có ý nghĩa.
- Không ghi metadata không có giá trị như `Base commit: UNKNOWN`.
- Một issue chỉ có một linked plan; cập nhật file hiện có thay vì tạo bản trùng.

### Metadata và section canonical

```markdown
# <ID> — <title>

Created: <YYYY-MM-DD>
Source issue: <verified GitHub link, hoặc ghi catalog sync đang pending>
Catalog: <relative Markdown link>
Phase plan: <relative Markdown link, hoặc N/A có lý do>
Status: Planned

## Objective

## Context

## Implementation plan

## Files/modules

## Dependencies / blocks

## Acceptance criteria

## Required tests / evidence

## Security and migration notes

## Out of scope

## Delivery evidence

### Implementation PRs

### Recorded commit/SHA references

## Definition of done
```

Với hồ sơ lịch sử, thay `Created` bằng `Issue closed`. Không tạo link, commit, approval,
date, test result hoặc evidence giả. `N/A` phải có lý do. Chỉ dùng `UNKNOWN` cho fact lịch
sử quan trọng đã cố gắng truy tìm nhưng không thể phục hồi.

### Lifecycle

- Tạo plan trước khi sửa code và link trực tiếp dưới field `Status` của catalog:

  ```markdown
  - **Plan file:** [<ID> detailed implementation plan](<relative-path>)
  ```

- Plan bắt đầu ở `Status: Planned`, chuyển `In progress` khi code bắt đầu, `Blocked` khi có
  blocker cụ thể, và `Done` chỉ sau khi Definition of Done đạt.
- Mỗi acceptance criterion phải map tới implementation location, test/manual verification,
  fixture/environment và expected evidence.
- Trong quá trình delivery, cập nhật plan bằng PR, final commit, command, environment và
  artifact thực tế; không ghi secret, token, customer content, PII hoặc corpus-bearing log.
- Reviewer độc lập phải kiểm tra issue, plan, diff, negative paths, security/ADR trigger và
  evidence. Author không phải verifier duy nhất.

## Prompt contract cho Cursor

Skill chịu trách nhiệm đọc và thực thi toàn bộ chuẩn trên. Prompt của user chỉ cần nêu:

1. mục tiêu hoặc issue ID;
2. constraint kinh doanh đặc biệt nếu có;
3. có hay không quyền tạo/sửa resource remote.

Ví dụ:

```text
Dùng issue-creator tạo draft issue cho hỗ trợ TXT UTF-16 trong core; chưa sync GitHub.
```

```text
Dùng issue-delivery triển khai P1A-11.
```

User không cần lặp lại format issue, format plan, quality gate, security review, reviewer
độc lập hay điều kiện `Done`; đó là trách nhiệm bắt buộc của skill.
