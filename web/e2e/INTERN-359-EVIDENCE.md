# Issue #359 — intern-21 E2E evidence

## Commands (local mock-mode)

Branch gợi ý: `intern/21-e2e-test-basics`.

```bash
pnpm install
pnpm --dir web exec playwright install chromium

# Bắt buộc nếu đang chạy `pnpm --dir web dev` (hoặc Vite khác) trên 4173 không có mock:
# Playwright local mặc định reuseExistingServer=true → dùng nhầm server, login/E2E fail.
lsof -ti :4173 | xargs kill -9 2>/dev/null   # optional: giải phóng port

CI=1 pnpm --dir web exec playwright test e2e/intern-upload-convert.spec.ts
CI=1 pnpm --dir web e2e
```

**Kỳ vọng khi chạy đúng:** spec intern **1 passed**; full mock suite **39 passed** (38 specs có sẵn + `intern-upload-convert.spec.ts`).

## Verification log (2026-08-10)

| Run | Environment | Result |
|-----|-------------|--------|
| `pnpm --dir web e2e` (no `CI=1`) | Vite non-mock đã bind 4173 | **38 failed** — không thấy `Email` / mock seams |
| `CI=1` + kill 4173 | Mock webServer OK; Cursor agent sandbox | **Browser launch fail** — Playwright tìm `chrome-headless-shell-mac-x64` trong khi cache host là **arm64** |
| Spec formatting | `prettier --check` | **Pass** |
| `CI=1 pnpm --dir web e2e` | Local arm64 Mac, mock webServer | **39 passed** (~28s) |

**Chẩn đoán login timeout (khi browser chạy được nhưng fail sớm):** thường do **reuse** dev server không có `VITE_MARKHAND_MOCK=1` — dùng `CI=1` hoặc tắt Vite trên 4173.

**Chạy trên máy dev (arm64 Mac):** dùng lệnh `CI=1` ở trên trong terminal ngoài sandbox; không cần sửa code nếu Playwright đã `install chromium` đúng kiến trúc.

## AC1 — Happy-path spec

- File: [`web/e2e/intern-upload-convert.spec.ts`](intern-upload-convert.spec.ts)
- Test: `upload converts a file and shows markdown preview`

## AC3 — Page object pattern and assertions

### Helpers vs locators (no class POM)

Shared steps live in [`web/e2e/support.ts`](support.ts): `login`, `openEmployeeHandbook`, `succeedUploadJob`, `advanceDocument`. The spec uses Playwright locators directly (`getByLabel`, `getByRole`, `getByTestId`).

### Assertions (user-visible behavior)

- `toBeVisible` on upload progress, conversion copy, document badge.
- `toContainText('Mock preview content for version')` on `document-preview-markdown` (mock fixture markdown, not raw upload bytes).
- `expect(jobId/documentId).toBeTruthy()` as API sanity checks captured from the routed upload replay.

### Upload routing

`POST /api/v1/uploads` uses `XMLHttpRequest`; the in-app fetch mock does not see it. The spec uses `page.route()` and replays the multipart body via in-page `fetch` so the mock store registers job and document (same as [`upload.spec.ts`](upload.spec.ts)).

### No arbitrary sleep

Waits use `expect(..., { timeout })` only.

## PR / issue comment checklist

- [x] ≥1 E2E happy path (`intern-upload-convert.spec.ts`)
- [x] Local pass evidence (`CI=1 pnpm --dir web e2e` → **39 passed**, 2026-08-10)
- [x] Giải thích POM helpers + assertions (section AC3 above)
