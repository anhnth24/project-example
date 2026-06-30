// Kiểu dữ liệu khớp với serde (camelCase) ở backend Rust.

export interface Workspace {
  id: string;
  name: string;
  path: string;
}

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
