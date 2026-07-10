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

export type AppView = "home" | "library" | "document" | "intelligence";
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

export type IntelligenceMode =
  | "handoff"
  | "quality"
  | "ask"
  | "versions"
  | "tables"
  | "privacy"
  | "export"
  | "watch";

export interface IntelligenceScope {
  sourceRels: string[];
}

export interface Citation {
  id: string;
  sourceRel: string;
  mdRel: string;
  heading: string;
  quote: string;
  start: number;
  end: number;
  page: number | null;
  confidence: number;
}

export interface CorpusChunk {
  id: string;
  sourceRel: string;
  mdRel: string;
  heading: string;
  text: string;
  start: number;
  end: number;
  page: number | null;
}

export interface SearchHit {
  chunk: CorpusChunk;
  snippet: string;
  score: number;
}

export interface AskResult {
  answer: string;
  citations: Citation[];
}

export interface QualityIssue {
  code: string;
  message: string;
  severity: "info" | "warning" | "error";
  start: number | null;
  end: number | null;
  recommendation: string;
}

export interface DocumentQuality {
  sourceRel: string;
  score: number;
  chars: number;
  headings: number;
  tables: number;
  issues: QualityIssue[];
}

export interface QualityReport {
  score: number;
  documents: DocumentQuality[];
  issueCount: number;
}

export type PiiKind = "email" | "phone" | "national_id" | "bank_account";

export interface PiiFinding {
  kind: PiiKind;
  text: string;
  sourceRel: string;
  start: number;
  end: number;
  confidence: number;
}

export interface PiiReport {
  findings: PiiFinding[];
  counts: Partial<Record<PiiKind, number>>;
}

export interface MarkdownTable {
  id: string;
  sourceRel: string;
  index: number;
  start: number;
  end: number;
  rows: string[][];
}

export interface SchemaField {
  name: string;
  fieldType: "string" | "number" | "date" | "boolean";
  examples: string[];
}

export interface DocumentSchema {
  sourceRel: string;
  headings: string[];
  fields: SchemaField[];
  tables: MarkdownTable[];
}

export interface VersionMeta {
  id: string;
  createdAt: number;
  bytes: number;
}

export interface VersionSnapshot {
  id: string;
  sourceRel: string;
  createdAt: number;
  markdown: string;
}

export interface DiffHunk {
  kind: "added" | "removed" | "modified" | "unchanged";
  oldStart: number;
  newStart: number;
  oldText: string;
  newText: string;
}

export interface MergeResult {
  markdown: string;
  conflicts: { index: number; ours: string; theirs: string }[];
}

export type WatchAction = "import_only" | "import_and_convert";

export interface WatchRule {
  id: string;
  watchAbs: string;
  targetFolderRel: string;
  pattern: string;
  action: WatchAction;
  enabled: boolean;
}

export interface WatchMatch {
  ruleId: string;
  sourceAbs: string;
  targetFolderRel: string;
  action: WatchAction;
}

export type HandoffMode = "deterministic" | "llm_assisted";

export interface HandoffItem {
  id: string;
  kind:
    | "business_requirement"
    | "functional_requirement"
    | "user_story"
    | "acceptance_criterion"
    | "test_case"
    | "glossary"
    | "assumption"
    | "open_question";
  text: string;
  citations: string[];
  status: string;
  parentId: string | null;
}

export interface HandoffValidation {
  ok: boolean;
  errors: { code: string; itemId: string | null; message: string }[];
  warnings: { code: string; itemId: string | null; message: string }[];
  citationCoverage: number;
  traceabilityCoverage: number;
}

export interface HandoffPack {
  schemaVersion: number;
  packId: string;
  productName: string;
  productSlug: string;
  locale: string;
  mode: HandoffMode;
  createdAt: number;
  sources: string[];
  citations: Citation[];
  items: HandoffItem[];
  traceability: unknown[];
  artifacts: Record<string, string>;
  validation: HandoffValidation;
}

export interface HandoffResult {
  pack: HandoffPack;
  outRelDir: string;
  llmNote: string | null;
}
