# Markhand desktop redesign — implementation plan

## 1. Decision and scope

The production target uses `re-design/project/Markhand - Thiết kế mới.dc.html`
(`PA-2a-DoiChieu-v2`) as the visual source of truth:

- dark LumiBase shell;
- 68 px icon rail and collapsible document drawer;
- document tabs;
- home, search/command palette, conversion queue, settings and drag/drop states;
- block comparison as the distinctive review workflow.

The implementation also carries forward the complete document modes from
`PA-3a-Workbench-v2`: side-by-side, Markdown and source. The comparison view is
added as a fourth mode. The Library grid from `PA-1c-ThuVien` becomes a rail
destination and reuses the same document/queue state.

`ExtensionDetail.dc.html` is a LumiBase marketplace example, not a Markhand
screen, and is outside this implementation.

## 2. Product invariants

1. The filesystem remains the source of truth. A source file is paired with
   `<source-name>.md`; no document database is introduced.
2. Existing path confinement, supported-extension checks, preview size gates,
   paired rename/delete behavior and settings persistence must remain intact.
3. No simulated conversion percentage. Jobs expose `queued`, `running`, `done`
   or `error`; running jobs use indeterminate progress until the core has real
   progress units.
4. Switching documents must never discard an unsaved Markdown draft.
5. The generated design runtime (`support.js`, `_ds_bundle.js`) is reference
   material only. Production UI is regular React and local CSS.
6. A “linked block” in the first release means the immutable Markdown produced
   by the last conversion linked to the editable Markdown block. The actual
   source file remains available in Side-by-side and Source modes. A future
   structured-core artifact can replace this snapshot without changing the UI
   contract.

## 3. Frontend architecture

### App shell

- `AppShell`: page background, rail, drawer and content region.
- `IconRail`: Home, Library, Documents, Search, Queue and Settings.
- `DocumentDrawer`: recursive DATA tree, filename filter and file operations.
- `DocumentTabs`: active/dirty states and guarded close.
- `HomeView`: redesigned empty/landing state.
- `LibraryView`: flattened document cards, status filter, selection and batch
  conversion.
- `CommandPalette`: filename search plus application commands.
- `ConvertQueue`: global job status popover.

### Document session state

Zustand owns sessions keyed by `relPath`:

```ts
interface DocumentSession {
  relPath: string;
  draft: string;
  baseline: string;
  loaded: boolean;
  dirty: boolean;
  mode: "compare" | "split" | "markdown" | "source";
  markdownTab: "edit" | "preview";
  savedAt: string | null;
}
```

The store also owns `openTabs`, `activeTab`, the current app destination,
dialogs, query state and conversion jobs. Only the active source preview is
mounted, so opening several large documents does not retain their source bytes.

### Block comparison

Markdown is split at ATX headings while preserving every byte of the original
string. If a document has no headings, paragraph groups are used. The immutable
baseline and editable draft are aligned by order and heading. Editing a block
updates the corresponding range in the full draft; saving still writes the
normal `.md` sidecar.

This provides an honest “conversion snapshot ↔ edited Markdown” review mode.
It does not claim page coordinates or audio timestamps that the current core
does not provide.

## 4. Conversion queue

The Tauri backend gains `import_file_only`, which validates and copies a file
into DATA without blocking on conversion. The frontend:

1. copies selected/dropped files;
2. refreshes the tree so raw files appear immediately;
3. enqueues their relative paths;
4. processes jobs serially through the existing `reconvert` command;
5. refreshes the tree and active session after each completion.

Serial execution is intentional: PDFium, Tesseract and Whisper are expensive,
and concurrent audio/PDF jobs can cause high memory use. The queue remains
responsive because Tauri conversion already runs through `spawn_blocking`.

## 5. Interaction details

- `Ctrl/Cmd+K`: open command palette.
- `Ctrl/Cmd+S`: save active Markdown draft.
- `Ctrl/Cmd+W`: request active-tab close.
- `Escape`: close the top overlay/dialog.
- Closing a dirty tab opens an in-app confirmation dialog.
- Rename/delete/create use in-app dialogs rather than browser
  `prompt()`/`confirm()`.
- Search, tab controls, tree rows and dialogs support keyboard interaction and
  visible focus.
- Functional icon targets are at least 44×44 px; icon visuals may remain 16 px.
- Reduced-motion users do not receive floating, spinning or glow animation
  beyond essential progress indication.

## 6. Visual implementation

- Copy the LumiBase token values into app-local semantic variables.
- Use Inter Variable already bundled by the app; do not load Google Fonts.
- Keep Lucide as the single icon family.
- Preserve white/light source-document surfaces inside the dark application.
- Reference viewport: 1440×900.
- At widths below 1100 px the drawer can collapse; at the configured minimum
  900×600 toolbars wrap or move secondary actions into compact controls.
- Faint LumiBase text is decorative only; labels and body copy must meet WCAG
  AA contrast.

## 7. Backend changes

- Add `import_file_only(folder_rel, source_abs) -> Node`.
- Extract a shared safe-copy helper used by both `import_file` and
  `import_file_only`.
- Keep `import_file` for compatibility.
- Ensure Settings continues to populate future `ConverterOptions` fields via
  `..Default::default()`.
- Add tests for copy-only import and retain path traversal/paired-file tests.

No core converter API change is required for this release. A later product-grade
source-anchor phase can add `StructuredDocument` with page/slide/sheet/time
anchors while keeping `Converter::convert_path` unchanged for CLI and MCP.

## 8. Delivery slices

1. Design tokens, app shell and reusable UI primitives.
2. Zustand tabs/sessions and unsaved-change protection.
3. Drawer search, command palette and file-operation dialogs.
4. Copy-only import plus serial conversion queue.
5. Workbench modes and snapshot-based block comparison.
6. Library grid and batch conversion.
7. Accessibility, responsive behavior and regression tests.

Each slice must preserve a usable app; existing conversion, editing, preview,
DATA-root and settings workflows cannot be deferred to a later slice.

## 9. Verification

- `pnpm build` for TypeScript and Vite.
- Rust unit tests for `fileconv-desktop`.
- Workspace tests once the repository toolchain can parse all locked
  Edition-2024 dependencies.
- Frontend tests for Markdown block round-trip, tree filtering, session dirty
  state and queue transitions.
- Manual acceptance at 1440×900 and 900×600:
  - upload/drop multiple files;
  - continue browsing while queue runs;
  - open/edit/switch/save multiple tabs;
  - close a dirty tab safely;
  - compare baseline blocks with edited blocks;
  - create/rename/delete folders and files;
  - change DATA root and conversion settings;
  - use all primary actions by keyboard.

## 10. Build baseline

The lockfile resolves Edition-2024 crates (including `time` 0.3.51), so the
workspace now pins Rust 1.88 through `rust-toolchain.toml` and declares the same
MSRV in Cargo metadata. This replaces the stale Rust 1.80 claim and makes local,
CI and Tauri builds use one compiler baseline.
