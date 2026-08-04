<!-- generated-done-issue-plan: P2-08 -->
# P2-08 — Upload progress và job lifecycle

Issue closed: 2026-07-27
Source issue: [#123](https://github.com/anhnth24/project-example/issues/123)
Catalog: [`backlog/phase-2/issues/README.md`](../markhand-web/backlog/phase-2/issues/README.md)
Phase plan: [`phase-2-web-spa.md`](../markhand-web/phase-2-web-spa.md)
Status: Done

## Objective

Not separately recorded in the compact catalog card. The recorded outcome is the issue title: **Upload progress và job lifecycle**.

## Context

- Phase: `2`.
- The catalog is the status authority.
- Historical status/evidence recorded by the catalog:

> #312. XHR progress thật, job SSE-nudge → `GET /jobs/{id}`. Không quan sát được stage index (API chỉ trả convert job id) — báo "converted, indexing tiếp server-side".
>
> **Cập nhật (badge tự cập nhật khi tài liệu đang xử lý, 2026-07-29 — owner critique:
> "trạng thái document chưa đúng giai đoạn xử lý khi load lại trang hoặc mở chức năng
> khác rồi quay lại"):** `LibraryPage` trước đó chỉ `refreshDocuments()` (refetch
> `GET /collections/{id}/documents`) khi có hành động rõ ràng (upload xong, bấm action)
> — một tài liệu worker đang chuyển `converting → converted → indexing → indexed` phía
> server đứng im trên UI tới khi F5. Đóng bằng cách poll đúng request đó (không chế
> transport mới, vẫn qua `useScopeSafeRequest` sẵn có) mỗi 5s khi trang hiện tại có ≥1
> tài liệu ở trạng thái non-terminal (`uploaded|converting|converted|indexing`); dừng khi
> hết non-terminal, tab ẩn (`document.visibilityState`/`visibilitychange`), rời trang,
> hoặc đổi org/collection (đã "miễn phí" theo scope-safety sẵn có của
> `documentsData`/`retainedDocuments`). Backoff 5s→15s→30s khi lỗi liên tiếp, reset khi
> thành công. Preview panel đang mở tài liệu đó cũng cập nhật theo (đọc lại từ cùng danh
> sách đã poll, không cần dây riêng). Mock seam: `__markhandMockDocs.advance(documentId)`
> (`components/library/testSupport.ts`'s `advanceDocumentState`, quy ước
> `__markhandMock*` sẵn có) tiến 1 bước forward-only
> (`uploaded→converting→converted→indexing→indexed`) — không tự chế "failed" (đó là một
> outcome thật, không phải bước tiến). Test: component `LibraryPage.test.tsx` (fake
> timers — bật/tắt theo non-terminal, tắt khi tab ẩn, backoff khi lỗi 429) + E2E mới
> `e2e/document-status-polling.spec.ts` (upload → converting → advance seam 3 lần →
> badge tự chuyển converted/indexing/indexed, không `page.reload()`/`page.goto()`).

## Implementation plan

Multipart/progress/cancel; job SSE; reconnect snapshot; accessible
status for uploaded→indexed/failed.

## Files/modules

The source catalog records implementation and file scope together; see **Implementation plan** above.

## Dependencies / blocks

P2-04/07.

## Acceptance criteria

Client/server progress distinct; recover
refresh; success/cancel/loss/gap/413/415/429/filename tests. **+ live status poll**:
xem cập nhật ở trên.

## Required tests / evidence

Client/server progress distinct; recover
refresh; success/cancel/loss/gap/413/415/429/filename tests. **+ live status poll**:
xem cập nhật ở trên.

## Security and migration notes

No client conversion queue.

## Out of scope

folder/watch/resumable protocol.

## Delivery evidence

### Implementation PRs

- [PR #312](https://github.com/anhnth24/project-example/pull/312) — Web: Organic design system, left rail shell, and library wave 2 (P2-07/08/09); merged `2026-07-27T05:59:33Z`

### Recorded commit/SHA references

- `461417bc700811e5ebb251ff76caac11c13cc07c`

- GitHub sync-closed timestamp: `2026-07-27T09:49:13Z` (recorded for traceability; not treated as the delivery date).

## Definition of done

- [x] The source catalog marks this issue `Done`.
- [x] Recorded acceptance/test/security/out-of-scope text is preserved above.
- [x] Missing historical facts are marked `UNKNOWN` rather than inferred.
- The plan records the issue scope and its existing delivery evidence.
