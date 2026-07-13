# Markhand recreation notes (distilled from project-example source)

## App structure (app/src)
- App.tsx: .app = flex row; Sidebar + main. selected file → DocView(key=relPath), else HomeState (.home > EmptyState + .home-steps 3 Cards padding=4 with .step-icon/.step-num/.step-title/.step-desc; steps: Tạo thư mục / Tải file gốc lên / Xem song song & sửa). Drag-drop overlay .drop-overlay > .drop-overlay-box (Upload 34, title "Thả để thêm vào thư mục đích", sub "PDF · Word · Excel · PPT · CSV · HTML · ảnh · audio"). Error toast .toast-wrap > Banner error dismissable, auto-hide 6s.
- Brand: .brand > .brand-mark "A→M" (bg #f47416) + .brand-name "Markhand".
- Sidebar: .data-card (Folder 14 amber icon, .data-label "THƯ MỤC DỮ LIỆU", .data-path rtl ellipsis, IconButton FolderCog 16 ghost sm) → .toolbar-row (.toolbar-grow Button primary sm "Tải file" Upload 15; IconButton secondary sm FolderPlus 15 "Thư mục mới"; IconButton secondary sm FilePlus2 15 "Markdown mới") → .dest-hint "Đích: <b>{folder}</b>" → .tree-scroll (Tree) → .sidebar-foot (Button ghost sm SettingsIcon 15 "Cài đặt convert").
- Tree row: .row (paddingLeft 8+depth*14) > .twisty (ChevronRight 14, .open rotates 90°) > .row-icon (Folder/FolderOpen 16 #e0a83e or fileIcon) > .row-label > .dot (if unconverted) > .row-actions (IconButton ghost sm Pencil 13, Trash2 13; visible on hover). Folder click toggles open + selects. depth<1 open default.
- DocView: .doc-toolbar = .doc-title (fileIcon 18 + name + .dirty-dot if dirty) | .doc-modes TabList (Song song/Markdown/File gốc) | .doc-actions (.doc-meta "N ký tự" + "Đã lưu HH:MM" success; Button ghost sm Copy "Copy MD"/"Đã copy" Check; Button primary sm Save "Lưu" disabled unless dirty; Button secondary sm RefreshCw "Convert lại"; Button ghost sm ExternalLink "Mở ngoài"). .doc-body.split/.md/.source > .pane.source-pane (border-right) + .pane.md-pane.
- MarkdownEditor: .md-editor > .md-tabs TabList sm (Soạn/Xem trước; default "Xem trước") > .cm-wrap (CodeMirror white, gutter #f8fafc #94a3b8, activeLine #eff6ff, 14px mono) or .md-preview.markdown-body.
- PDF preview: .preview.pdf-canvas (bg #e9edf3 padding 16) > .pdf-pages (column center gap 14) > .pdf-page (white, radius 4, shadow-sm).
- Settings Dialog width 520: title "Cài đặt convert"; .settings-form gap 16: TextInput "Ngôn ngữ OCR (ảnh / PDF scan)" val "vie+eng"; Checkbox "OCR trang PDF dạng scan (ít/không có lớp text)" checked; Checkbox "OCR thêm ảnh nhúng trong trang PDF có text (chậm hơn)" unchecked; .settings-grid (TextInput "Ngôn ngữ audio" "vi" + NumberInput "Thread audio" 4); .settings-whisper (TextInput whisper model placeholder "đường dẫn tới ggml-*.bin" + Button secondary "Chọn…"); footer divider + Hủy ghost / Lưu primary.
- fileIcon colors: pdf FileType2 #e5484d; docx FileText #2f6fed; pptx Presentation #e8833a; xlsx/csv FileSpreadsheet #1f9d57; html Globe #5b6cff; image FileImage #8a63d2; audio FileAudio #d6409f; markdown FileText #6b7280; other File #9aa0aa.

## Astryx neutral theme (light) — resolved values
font-family-body: Figtree,-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif (Figtree NOT bundled → system). font-size-base 14px; label 14/500; supporting 12/1.667; large 17/600.
accent #262626; on-accent #fff; neutral(secondary btn bg) rgba(0,0,0,0.06); overlay-hover rgba(0,0,0,0.05); overlay-pressed rgba(0,0,0,0.10); text-primary #171717; text-secondary #737373; text-disabled #a3a3a3; border #ebebeb; border-emphasized #d4d4d4; bg-body #f1f1f1; bg-surface/card #fff; overlay(modal) rgba(0,0,0,0.5); error-muted #facecb; text-red #89001a.
radius: inner 6px, element 10px, container 12px. sizes: sm 28px, md 32px, lg 36px.
Button: inline-flex center gap 8, padding-inline 12, radius 10, font 14/500, border 0. primary bg #262626 white; secondary bg rgba(0,0,0,0.06) #171717; ghost transparent #171717; hover = overlay-hover layer; icon 16.
IconButton: square (28 sm / 32 md), same variants, radius 10.
TabList: flex gap 2px, border-bottom 1px #ebebeb (full width). Tab: height 32 (md)/28(sm), padding-inline 12, gap 4, radius 10 (hover bg overlay), font 14/400 #737373; selected #171717 600 + 2px indicator bg #262626 radius-full at bottom; icon 14-16.
Card: bg #fff, radius 12, border 1px transparent, padding 16 (padding=4).
EmptyState: column center, text-align center, gap 16, padding 32/24; icon lg 20+ secondary; title 17/600 #171717; desc 14 #737373 maxW 360.
Banner error: radius 12, padding 12/16, flex gap 8, bg #facecb, title 14/600 #89001a, dismiss X.
Dialog: overlay rgba(0,0,0,0.5); panel white radius ~16 (page/container), shadow high, width 520, header title+X, footer divider.
TextInput: label 14/500 #171717 gap 4; control height 32, radius 10, border 1px #d4d4d4, padding-inline 10, font 14; placeholder #737373.
Checkbox: 20×20, radius 6, border 1px #d4d4d4; checked bg #262626 + white check.

## Lucide icon paths (v1.22, 24×24, stroke currentColor 2, round/round)
file-text: M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z | M14 2v5a1 1 0 0 0 1 1h5 | M10 9H8 | M16 13H8 | M16 17H8
file(base outline)= first two of file-text.
file-spreadsheet: base + M8 13h2 | M14 13h2 | M8 17h2 | M14 17h2
file-image: base + circle cx10 cy12 r2 + m20 17-1.296-1.296a2.41 2.41 0 0 0-3.408 0L9 22
file-audio: M4 6.835V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.706.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2h-.343 | M14 2v5a1 1 0 0 0 1 1h5 | M2 19a2 2 0 0 1 4 0v1a2 2 0 0 1-4 0v-4a6 6 0 0 1 12 0v4a2 2 0 0 1-4 0v-1a2 2 0 0 1 4 0
file-type-2: M12 22h6a2 2 0 0 0 2-2V8a2.4 2.4 0 0 0-.706-1.706l-3.588-3.588A2.4 2.4 0 0 0 14 2H6a2 2 0 0 0-2 2v6 | M14 2v5a1 1 0 0 0 1 1h5 | M3 16v-1.5a.5.5 0 0 1 .5-.5h7a.5.5 0 0 1 .5.5V16 | M6 22h2 | M7 14v8
file-plus-2: M11.35 22H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.706.706l3.588 3.588A2.4 2.4 0 0 1 20 8v5.35 | M14 2v5a1 1 0 0 0 1 1h5 | M14 19h6 | M17 16v6
file-warning: base + M12 9v4 | M12 17h.01
folder: M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z
folder-open: m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2
folder-plus: folder + M12 10v6 | M9 13h6
folder-cog: M10.3 20H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.98a2 2 0 0 1 1.69.9l.66 1.2A2 2 0 0 0 12 6h8a2 2 0 0 1 2 2v3.3 | m14.305 19.53.923-.382 | m15.228 16.852-.923-.383 | m16.852 15.228-.383-.923 | m16.852 20.772-.383.924 | m19.148 15.228.383-.923 | m19.53 21.696-.382-.924 | m20.772 16.852.924-.383 | m20.772 19.148.924.383 | circle cx18 cy18 r3
upload: M12 3v12 | m17 8-5-5-5 5 | M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4
columns-2: rect 18×18 x3 y3 rx2 | M12 3v18
settings: M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915 | circle cx12 cy12 r3
chevron-right: m9 18 6-6-6-6
pencil: M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z | m15 5 4 4
trash-2: M10 11v6 | M14 11v6 | M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6 | M3 6h18 | M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2
save: M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z | M17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7 | M7 3v4a1 1 0 0 0 1 1h7
refresh-cw: M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8 | M21 3v5h-5 | M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16 | M8 16H3v5
external-link: M15 3h6v6 | M10 14 21 3 | M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6
copy: rect 14×14 x8 y8 rx2 ry2 | M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2
check: M20 6 9 17l-5-5
presentation: M2 3h20 | M21 3v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V3 | m7 21 5-5 5 5
globe: circle cx12 cy12 r10 | M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20 | M2 12h20
loader-circle: M21 12a9 9 0 1 1-6.219-8.56
x: M18 6 6 18 | m6 6 12 12
search: m21 21-4.34-4.34 | circle cx11 cy11 r8
plus: M5 12h14 | M12 5v14

## App README facts
Tauri 2 + React desktop app for BA/PM. DATA root folder mapped; folder = real subfolder; document = (file gốc, file .md) cạnh nhau (report.pdf → report.pdf.md). Formats: pdf docx pptx xlsx/xls/ods csv html image audio. pptx: no in-app preview → "Mở ngoài". Big-file guard; text 512KB cap; excel 1000 rows cap. Missing (planned): drag-drop ✓(has), đa tab, tìm kiếm, đóng gói. Sidebar 286px min 250. Toolbar 72? no. App styles.css palette: bg #f1f5f9, panel #fff, sidebar #f8fafc, border #e2e8f0/#cbd5e1, text #0f172a, muted #64748b, faint #94a3b8, accent #2563eb hover #1d4ed8 soft #eff6ff, amber #f59e0b (folder icon #e0a83e), success #059669, danger #dc2626, radius 10/7, font Inter + Plus Jakarta Sans display.
