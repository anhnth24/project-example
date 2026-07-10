// Kiểu dữ liệu khớp với serde (camelCase) ở backend Rust.

export interface FsNode {
  name: string;
  relPath: string;
  isDir: boolean;
  /** "folder" | pdf/docx/pptx/xlsx/csv/html/image/audio | "markdown" | "other" */
  kind: string;
  supported: boolean;
  mdRelPath: string | null;
  standaloneMd: boolean;
  children: FsNode[];
}

export interface Settings {
  ocrLangs: string;
  pdfOcr: boolean;
  pdfOcrImages: boolean;
  audioLang: string;
  audioThreads: number;
  whisperModel: string | null;
}

export type AppView = "home" | "library" | "document";
export type DocumentMode = "compare" | "split" | "markdown" | "source";
export type MarkdownTab = "edit" | "preview";

export interface DocumentSession {
  relPath: string;
  baseline: string;
  savedDraft: string;
  draft: string;
  loaded: boolean;
  dirty: boolean;
  saving: boolean;
  revision: number;
  mode: DocumentMode;
  markdownTab: MarkdownTab;
  savedAt: string | null;
}

export type ConvertJobStatus = "queued" | "running" | "done" | "error";

export interface ConvertJob {
  id: string;
  relPath: string;
  name: string;
  kind: string;
  status: ConvertJobStatus;
  error: string | null;
  queuedAt: number;
}
