# Brainstorm: Version management + auto-update cho Markhand

- Date: 2026-07-11
- Status: approved
- Related: PR #29 (`.github/workflows/release-desktop.yml` — CI build installer, KHÔNG làm lại)
- Next step: GitHub issue (user chọn issue thay vì plan ngay)

## Problem

- App chưa có quản lý version: `0.1.0` hardcode ở `app/src-tauri/tauri.conf.json` + `app/src-tauri/Cargo.toml`, không hiển thị trong app, không có quy trình bump/release.
- Chưa có cơ chế update: user cài installer từ CI artifact (hết hạn 90 ngày, cần login GitHub), không biết khi nào có bản mới.
- CI (PR #29) build msi/nsis/dmg/deb/appimage nhưng chỉ upload artifact — chưa có GitHub Release, chưa có URL public ổn định.

## Decisions (user-confirmed)

| Câu hỏi | Quyết định |
|---|---|
| Repo access | Public → dùng thẳng `releases/latest/download/latest.json` |
| Release trigger | Chỉ tag `markhand-vX.Y.Z` tạo Release; push master vẫn build artifact test |
| Update UX | Auto-check khi mở app (toggle trong Settings, mặc định bật) + nút check thủ công; hỏi trước khi cài |
| Platforms | Windows (nsis) + macOS + Linux AppImage; `.deb` chỉ thông báo + link tải |
| CI approach | **B** — giữ nguyên workflow PR #29, thêm release job (không dùng tauri-action) |

## Approaches evaluated

**A. tauri-apps/tauri-action** — tự lo Release + latest.json. Loại: phải viết lại logic signing có điều kiện (`vars.*_SIGNING_ENABLED`, `--no-sign`, verify Authenticode/notarization) của PR #29.

**B. Mở rộng workflow hiện có (CHỌN)** — diff nhỏ nhất, giữ signing/verify. Chi phí: tự maintain ~30 dòng script lắp `latest.json`.

## Final design

### 1. Version — 1 nguồn sự thật
- `tauri.conf.json` `version` là canonical (Tauri override Cargo.toml).
- Quy trình release: bump version trong `tauri.conf.json` → commit → tag `markhand-vX.Y.Z` → push tag.
- CI validate tag == version trong conf, lệch → fail sớm. Không tool bump/changelog tự động (YAGNI).

### 2. Updater plumbing
- Thêm `tauri-plugin-updater` + `tauri-plugin-process` (relaunch).
- Keypair minisign (`pnpm tauri signer generate`): private key → GitHub secret `TAURI_SIGNING_PRIVATE_KEY` (+ password nếu đặt), pubkey → `tauri.conf.json` `plugins.updater.pubkey`.
- Endpoint: `https://github.com/anhnth24/project-example/releases/latest/download/latest.json`.
- Capabilities: `updater:default`, `process:allow-restart`.
- `bundle.createUpdaterArtifacts: true` → build sinh thêm update artifact + `.sig`.

### 3. CI (mở rộng PR #29, không đổi bước build)
- Job build: thêm env `TAURI_SIGNING_PRIVATE_KEY` khi build tag.
- Job `release` mới, chạy khi `startsWith(github.ref, 'refs/tags/markhand-v')`:
  1. Validate tag khớp version `tauri.conf.json`.
  2. Download artifacts 3 OS.
  3. Script lắp `latest.json` (version, notes, pub_date, per-platform `{url, signature}` cho `windows-x86_64`, `darwin-aarch64`, `linux-x86_64`).
  4. `gh release create markhand-vX.Y.Z` đính installer + update artifacts + latest.json. Cần `permissions: contents: write` cho job này.

### 4. Settings UI (`app/src/components/Settings.tsx`)
- Hiện version app (`getVersion()` từ `@tauri-apps/api/app`).
- Toggle "Tự kiểm tra cập nhật khi mở app" — lưu vào settings store hiện có, mặc định bật.
- Nút "Kiểm tra cập nhật": có bản mới → hiện version + release notes → user bấm Update → download (progress) → install → nhắc restart.
- Linux non-AppImage (deb): chỉ thông báo bản mới + link Release page.

## Risks

- **Mất private key minisign = mất khả năng push update cho user cũ** → backup ngoài GitHub secret ngay khi sinh key.
- macOS chưa ký: updater vẫn hoạt động (minisign độc lập Authenticode/notarization) nhưng Gatekeeper vẫn cảnh báo như hiện tại — không tệ hơn status quo.
- Bản đầu tiên (`markhand-v0.1.0`) user tự tải; auto-update có tác dụng từ bản sau.
- NSIS updater cần thoát app khi cài — dùng flow mặc định của plugin (hỏi user rồi restart).

## Success criteria

1. Settings hiển thị đúng version từ `tauri.conf.json`.
2. Push tag `markhand-vX.Y.Z` → GitHub Release có installer + update artifacts + `latest.json` hợp lệ.
3. App bản cũ (Win/Mac/AppImage) check thấy bản mới, cài và restart thành công.
4. Toggle auto-check tắt → không check khi mở app; nút thủ công vẫn chạy.
5. Tag lệch version conf → CI fail trước khi build.

## Unresolved questions

- macOS: chỉ build `aarch64` (macos-latest) — có cần Intel (`darwin-x86_64`) không? Mặc định: chưa (YAGNI, thêm sau nếu có user Intel).
- Release notes lấy từ đâu: mặc định auto-generate của GitHub (`--generate-notes`); viết tay sau nếu cần.
