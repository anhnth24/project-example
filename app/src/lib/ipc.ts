// Lớp bọc duy nhất gọi sang backend Rust. Tauri tự đổi key camelCase (JS) -> snake_case (Rust).
import { invoke } from "@tauri-apps/api/core";
import type {
  AskResult,
  DiffHunk,
  DocumentSchema,
  FsNode,
  HandoffMode,
  HandoffResult,
  LlmConnectionResult,
  LlmProviderPreset,
  MarkdownTable,
  MergeResult,
  PiiReport,
  Project,
  QualityReport,
  SearchHit,
  Settings,
  VersionMeta,
  VersionSnapshot,
  WatchMatch,
  WatchRule,
  ImportFolderResult,
} from "./types";

export const api = {
  supportedExtensions: () => invoke<string[]>("supported_extensions"),

  getDataRoot: () => invoke<string>("get_data_root"),
  setDataRoot: (path: string) => invoke<string>("set_data_root", { path }),

  readTree: () => invoke<FsNode>("read_tree"),

  createFolder: (parentRel: string, name: string) =>
    invoke<void>("create_folder", { parentRel, name }),
  createMarkdown: (parentRel: string, name: string) =>
    invoke<FsNode>("create_markdown", { parentRel, name }),
  renameNode: (relPath: string, newName: string) =>
    invoke<void>("rename_node", { relPath, newName }),
  deleteNode: (relPath: string) => invoke<void>("delete_node", { relPath }),

  importFile: (folderRel: string, sourceAbs: string) =>
    invoke<FsNode>("import_file", { folderRel, sourceAbs }),
  importFileOnly: (folderRel: string, sourceAbs: string) =>
    invoke<FsNode>("import_file_only", { folderRel, sourceAbs }),
  reconvert: (sourceRel: string) => invoke<string>("reconvert", { sourceRel }),

  readTextFile: (relPath: string) => invoke<string>("read_text_file", { relPath }),
  writeTextFile: (relPath: string, content: string) =>
    invoke<void>("write_text_file", { relPath, content }),
  readTextPreview: (relPath: string, maxBytes: number) =>
    invoke<{ text: string; truncated: boolean; size: number }>("read_text_preview", {
      relPath,
      maxBytes,
    }),
  fileSize: (relPath: string) => invoke<number>("file_size", { relPath }),

  resolvePath: (relPath: string) => invoke<string>("resolve_path", { relPath }),
  /** Bytes thô của file (ArrayBuffer) cho pdf.js/docx-preview/SheetJS. */
  readBytes: (relPath: string) => invoke<ArrayBuffer>("read_bytes", { relPath }),

  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  listProjects: () => invoke<Project[]>("list_projects"),
  createProject: (name: string) =>
    invoke<Project>("create_project", { req: { name } }),
  adoptProject: (folderRel: string, name?: string | null) =>
    invoke<Project>("adopt_project", { req: { folderRel, name } }),
  importLocalFolder: (
    projectId: string,
    sourceAbs: string,
    targetFolderRel?: string | null,
  ) =>
    invoke<ImportFolderResult>("import_local_folder", {
      req: { projectId, sourceAbs, targetFolderRel },
    }),
  removeProject: (projectId: string, deleteContents = false) =>
    invoke<void>("remove_project", { req: { projectId, deleteContents } }),
  getLlmProviderPresets: () =>
    invoke<LlmProviderPreset[]>("get_llm_provider_presets"),
  testLlmConnection: () => invoke<LlmConnectionResult>("test_llm_connection"),

  generateHandoffPack: (req: {
    sourceRels: string[];
    productName: string;
    productSlug: string;
    mode: HandoffMode;
    outRelDir?: string | null;
  }) => invoke<HandoffResult>("generate_handoff_pack", { req }),
  readHandoffArtifact: (relPath: string) =>
    invoke<string>("read_handoff_artifact", { req: { relPath } }),
  saveHandoffArtifact: (outRelDir: string, name: string, content: string) =>
    invoke<void>("save_handoff_artifact", {
      req: { outRelDir, name, content },
    }),
  exportExistingHandoff: (outRelDir: string, outputAbs: string) =>
    invoke<string>("export_existing_handoff", {
      req: { outRelDir, outputAbs },
    }),
  runQualityReport: (sourceRels: string[]) =>
    invoke<QualityReport>("run_quality_report", { req: { sourceRels } }),
  searchIntelligence: (sourceRels: string[], query: string, limit = 20) =>
    invoke<SearchHit[]>("search_intelligence", { req: { sourceRels, query, limit } }),
  askIntelligence: (
    sourceRels: string[],
    question: string,
    topK = 6,
    useLlm = false,
  ) =>
    invoke<AskResult>("ask_intelligence", {
      req: { sourceRels, question, topK, useLlm },
    }),
  scanPii: (sourceRels: string[]) =>
    invoke<PiiReport>("scan_pii", { req: { sourceRels } }),
  redactPii: (sourceRel: string) =>
    invoke<{ report: PiiReport; redactedRelPath: string }>("redact_pii", {
      req: { sourceRel },
    }),
  hardOcrImage: (sourceRel: string) =>
    invoke<{ markdown: string; artifactRelPath: string }>("hard_ocr_image", {
      req: { sourceRel },
    }),
  extractDocumentSchema: (sourceRels: string[]) =>
    invoke<DocumentSchema[]>("extract_document_schema", { req: { sourceRels } }),
  listMarkdownTables: (sourceRel: string) =>
    invoke<MarkdownTable[]>("list_markdown_tables", { req: { sourceRel } }),
  updateMarkdownTable: (sourceRel: string, tableId: string, rows: string[][]) =>
    invoke<{ mdRelPath: string; markdown: string }>("update_markdown_table", {
      req: { sourceRel, tableId, rows },
    }),
  exportMarkdownTable: (
    sourceRel: string,
    tableId: string,
    rows: string[][],
    outputAbs: string,
  ) =>
    invoke<string>("export_markdown_table", {
      req: { sourceRel, tableId, rows, outputAbs },
    }),
  snapshotDocumentVersion: (sourceRel: string) =>
    invoke<VersionMeta>("snapshot_document_version", { req: { sourceRel } }),
  listDocumentVersions: (sourceRel: string) =>
    invoke<VersionMeta[]>("list_document_versions", { req: { sourceRel } }),
  readDocumentVersion: (sourceRel: string, versionId: string) =>
    invoke<VersionSnapshot>("read_document_version", {
      req: { sourceRel, versionId },
    }),
  diffDocumentVersions: (
    sourceRel: string,
    oldVersionId: string,
    newVersionId: string,
  ) =>
    invoke<DiffHunk[]>("diff_document_versions", {
      req: { sourceRel, oldVersionId, newVersionId },
    }),
  mergeDocumentVersions: (base: string, ours: string, theirs: string) =>
    invoke<MergeResult>("merge_document_versions", { req: { base, ours, theirs } }),
  getWatchRules: () => invoke<WatchRule[]>("get_watch_rules"),
  setWatchRules: (rules: WatchRule[]) =>
    invoke<void>("set_watch_rules", { req: { rules } }),
  scanWatchRules: () => invoke<WatchMatch[]>("scan_watch_rules"),
  exportKnowledgePack: (req: {
    sourceRels: string[];
    productName: string;
    productSlug: string;
    outputAbs: string;
  }) => invoke<string>("export_knowledge_pack", { req }),
};
