# Intern Tasks Guide — Markhand (file → Markdown)

> Mỗi task: **Hướng làm** (định hướng, KHÔNG code mẫu — tự viết), **Cần tìm hiểu**, **File đụng**, **Tiêu chí xong**, **Cạm bẫy**.
> Nguyên tắc: YAGNI/KISS/DRY; chính xác tiếng Việt > format; chỉ đụng file được giao.

## Sơ đồ sở hữu file (tránh đụng nhau khi làm song song)
- **A (Rust):** `crates/core/src/{viet_legacy.rs, conv/*, lib.rs}` + `src-tauri/src/lib.rs` (chỉ `SUPPORTED_EXTS`) + comment `app/src/lib/types.ts`.
- **B (Frontend):** `app/src/components/{MarkdownEditor.tsx, DocView.tsx, Settings.tsx}` + `styles.css`.
- **C (Frontend):** `app/src/components/{Sidebar.tsx, Tree.tsx}` + `app/src/state/store.ts`.
- Chú ý: `DocView.tsx` có 2 task (B2, B3) → B làm **liền nhau**. `types.ts` chỉ A2 sửa comment → báo B4 tránh đụng.

## Lệnh nền (chạy trước khi làm)
```bash
cargo build --release && cargo test
cd app && pnpm tauri dev     # chạy app để test UI
./target/release/fileconv one <file>   # test 1 file qua CLI
```

---

## A — BACKEND RUST

### A1. Decode bảng mã VNI — `crates/core/src/viet_legacy.rs`
**Hướng làm:** Bám đúng pattern TCVN3 đã có (`TCVN3_MAP`, `tcvn3_char()`, `looks_like_tcvn3()`, `decode_tcvn3()`). Tạo bản song song cho VNI: `VNI_MAP`, `vni_char()`, `looks_like_vni()`, `decode_vni()`. Mở rộng `decode_text()` thêm nhánh: không-UTF-8 + `looks_like_vni` → decode VNI.
**Cần tìm hiểu:**
- Spec **VNI-Windows**: 1-byte (0x80–0xFF) hay composite 2-byte? Tìm bảng VNI→Unicode tin cậy (vietunicode.sourceforge.net; verify bằng tool chuyendoi.com).
- `binary_search_by_key` yêu cầu MAP sort theo byte key.
- **Heuristic phân biệt TCVN3 vs VNI** (cùng 1-byte, byte >0x7F): tìm byte đặc trưng chỉ có ở 1 bảng để chọn ưu tiên.
**Tiêu chí xong:** file mã VNI → ra tiếng Việt đúng; `looks_like_vni` KHÔNG nhận nhầm UTF-8; ≥2 unit test (1 câu đúng + 1 anti-test UTF-8). `cargo test -p fileconv-core` pass.
**Cạm bẫy:** nhiều bảng VNI copy trên web sai byte → cross-check ≥2 nguồn; tuyệt đối không decode nhầm UTF-8 hợp lệ (đã có test mẫu `utf8_not_flagged`).

### A2. Converter `.txt` mới — `conv/txt.rs`(mới) + `conv/mod.rs` + `lib.rs` + `src-tauri/src/lib.rs` + comment `app/src/lib/types.ts`
**Hướng làm:**
1. Tạo `conv/txt.rs`: `pub fn to_markdown(path: &Path) -> Result<String, ConvertError>` → đọc bytes → `crate::viet_legacy::decode_text(bytes)` (tự xử lý UTF-8/TCVN3) → trả về.
2. `conv/mod.rs`: thêm `pub mod txt;`.
3. `lib.rs`: thêm variant `FormatKind::Txt`; thêm `"txt"` vào `from_path`; thêm arm `as_str`.
4. `convert_path`: thêm routing `FormatKind::Txt => conv::txt::to_markdown(path)`.
5. `src-tauri/src/lib.rs`: thêm `"txt"` vào `SUPPORTED_EXTS` (để upload filter + cây hiện `supported`).
6. Cập nhật comment liệt kê kind ở `types.ts`.
**Cần tìm hiểu:** đọc `conv/csv_conv.rs` (converter đơn giản nhất làm mẫu); hiểu `FormatKind::from_path` + routing trong `convert_path`; hiểu `SUPPORTED_EXTS` tác động tới UI (filter upload, flag `supported`, icon).
**Tiêu chí xong:** `fileconv one file.txt` ra đúng nội dung (kể cả .txt mã TCVN3); app upload .txt được và hiện nút Convert.
**Cạm bẫy:** `.txt` thuần thì `decode_text` đã đủ — KHÔNG thêm dependency. Muốn `.epub`/`.rtf` để task khác.

### A3. Khóa test cho `docx.rs` & `html.rs` — `conv/docx.rs`, `conv/html.rs`
**Hướng làm:** Đọc 2 converter → viết unit test trong block `#[cfg(test)]` của từng file: tạo file mẫu nhỏ trong temp dir, convert, assert nội dung. Mục tiêu **khóa behavior** (test fail khi logic đổi sai), không phải cover 100%.
**Cần tìm hiểu:** đọc test mẫu trong `viet_legacy.rs` và `lib.rs` (`output_normalized_to_nfc`) — cách tạo temp file + assert; hiểu Rule 6 (test encode WHY: docx→heading `#`, html→bỏ `<script>`).
**Tiêu chí xong:** mỗi converter ≥2 test nhỏ (happy path + 1 edge: html có `<script>`, docx có bảng/heading). `cargo test -p fileconv-core` pass.
**Cạm bẫy:** đừng test chi tiết formatting thừa — test ý định; dùng temp dir, dọn dẹp sau test.

---

## B — FRONTEND (editor + DocView + settings)

### B1. Toolbar format + Find/Replace — `MarkdownEditor.tsx`
**Hướng làm:**
- Thêm thanh công cụ (B/I/H/list/link) chỉ ở tab "Soạn": click → chèn cú pháp markdown quanh vùng chọn (qua API selection của CodeMirror).
- Find/Replace (Ctrl+F / Ctrl+H): ô tìm + thay, nhảy match. Dùng extension `@codemirror/search` sẵn có.
**Cần tìm hiểu:** CodeMirror 6 (`@uiw/react-codemirror`): dispatch transaction để thay text quanh selection; `@codemirror/search` (`openSearchPanel`, `searchKeymap`).
**Tiêu chí xong:** B/I/H chèn đúng cú pháp quanh chữ chọn; Ctrl+F mở tìm + thay thế hoạt động.
**Cạm bẫy:** **Ctrl+S đã có ở `DocView.tsx` — KHÔNG làm lại**; toolbar chỉ hiện tab "Soạn".

### B2. Progress/ước tính khi convert — `DocView.tsx`
**Hướng làm:** Hiện trạng `convert()` chỉ set `converting=true` rồi đợi. Cải thiện UX: spinner rõ + ước tính. Lấy `api.fileSize(node.relPath)` trước convert → nếu lớn → gợi ý "file lớn, có thể mất vài giây". *(Stretch — phức tạp hơn: backend emit Tauri event % tiến trình từ `convert_and_write_md`, UI `listen`.)*
**Cần tìm hiểu:** đọc `DocView.convert()`, `api.fileSize`, `api.reconvert`; Tauri events `emit`/`listen` (nếu làm stretch).
**Tiêu chí xong:** convert file lớn có phản hồi rõ, không treo im; nút "Convert lại" loading đúng.
**Cạm bẫy:** backend đang `spawn_blocking` 1 lần → % thật phải đổi backend, cân nhắc có đáng không (chỉ làm stretch nếu còn thời gian).

### B3. Export MD → HTML/DOCX — `DocView.tsx` (làm sau B2)
**Hướng làm:** Thêm nút "Export" cạnh "Copy MD" → chọn định dạng → Tauri `save()` → ghi file. HTML: render markdown→HTML (`remark`+`remark-html`, gói CSS tối thiểu). DOCX (stretch): cần lib sinh docx.
**Cần tìm hiểu:** Tauri `@tauri-apps/plugin-dialog` `save()`, `writeTextFile`/ghi bytes; `remark`/`remark-html` (đã có `react-markdown`+`remark-gfm`).
**Tiêu chí xong:** export `.html` mở đúng (định dạng + bảng GFM). DOCX optional.
**Cạm bẫy:** thêm dependency phải cân nhắc (Rule 2); HTML thường đủ cho BA/PM.

### B4. Settings: validation + reset — `Settings.tsx`
**Hướng làm:** Validate trước Lưu: `audioThreads` 1–32, `ocrLangs` không rỗng + đúng dạng (`vie+eng`), `audioLang` không rỗng → sai hiện lỗi + disable nút Lưu. Thêm nút "Khôi phục mặc định" → set form về `DEFAULTS` (đã có).
**Cần tìm hiểu:** đọc `Settings.tsx` (state qua `setForm`); component `NumberInput`/`TextInput` của `@astryxdesign/core`; chọn thời điểm validate (onChange vs onSubmit).
**Tiêu chí xong:** nhập sai không lưu + báo rõ; reset trả đúng default.
**Cạm bẫy:** `NumberInput` có min/max rồi nhưng vẫn phải validate giá trị rỗng/NaN.

---

## C — FRONTEND (tree + search + UX)

### C1. Search (lọc cây + tìm trong MD) — `Sidebar.tsx` + `Tree.tsx` + `store.ts`
**Hướng làm:** Ô search ở Sidebar: gõ → lọc cây theo tên (match substring, đệ quy giữ thư mục cha nếu con khớp). "Tìm trong MD": tìm text trong `.md` (đơn giản nhất: chỉ file đang mở; multi-file cần backend → stretch).
**Cần tìm hiểu:** đọc `Tree.tsx` (render đệ quy), `store.ts` (`tree`, `findByRel`); thuật toán lọc cây đệ quy giữ cấu trúc cha-con.
**Tiêu chí xong:** gõ tên → cây thu hẹp đúng; xóa/Esc → đầy đủ.
**Cạm bẫy:** đừng làm multi-file search nặng nếu chưa cần — confirm phạm vi với lead.

### C2. Thay `prompt()`/`confirm()` native bằng Dialog — `Tree.tsx` + `Sidebar.tsx`
**Hướng làm:** `Tree.tsx` đang dùng `confirm()` (xóa) + `prompt()` (đổi tên); `Sidebar.tsx` dùng `prompt()` (tạo folder/markdown). Thay bằng `Dialog` của `@astryxdesign/core` (đã dùng ở Settings). Rename: dialog có input + OK/Hủy. Delete: dialog xác nhận.
**Cần tìm hiểu:** đọc `Settings.tsx` xem `Dialog`/`Layout`/`LayoutFooter` xài; quản lý state "dialog mở + node đang thao tác".
**Tiêu chí xong:** hết prompt/confirm native; gõ tiếng Việt trong input OK; lỗi hiển thị trong dialog.
**Cạm bẫy:** giữ `e.stopPropagation()` ở nút icon để không select node khi click.

### C3. Sort + filter cây — `Sidebar.tsx` + `Tree.tsx` + `store.ts`
**Hướng làm:** Thêm controls nhỏ: sắp xếp (tên/loại/trạng thái convert) + filter nhanh (chỉ file chưa convert). Áp dụng khi render cây.
**Cần tìm hiểu:** đọc `build_tree` ở backend (đã sort folder-trước-rồi-tên) — sort UI là lớp thêm bên trên; Zustand selector.
**Tiêu chí xong:** đổi sort → cây sắp lại; filter "chưa convert" → chỉ hiện file chưa có `.md`.
**Cạm bẫy:** KHÔNG sort trên backend (đổi contract) — sort ở UI.

---

## Phân công & thứ tự
- **A:** A1 → A2 → A3
- **B:** B1 → B2 → B3 → B4 (B2, B3 cùng `DocView.tsx` nên liền nhau)
- **C:** C1 → C2 → C3

## Câu hỏi mở cho lead
1. 3 bạn đã biết **Rust** chưa? Chưa → A làm task Rust nhẹ nhất (A2 `.txt` + A3 test), phần nặng (A1 VNI) người hướng dẫn kèm.
2. Có nên expose các option backend chưa có UI (`pdf_pages`, `xlsx_sheet`, `max_chars`) ra Settings không? (đang để backlog — nếu có thì thành task B phụ).
