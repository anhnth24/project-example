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

export interface PptxPreviewMeta {
  slideCount: number;
  widthEmu: number;
  heightEmu: number;
}

interface PptxShapeBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type PptxPreviewShape =
  | (PptxShapeBounds & {
      kind: "text";
      text: string;
      fontPt: number;
      bold: boolean;
      color: string;
      fill: string | null;
    })
  | (PptxShapeBounds & {
      kind: "image";
      alt: string;
      dataUrl: string;
    })
  | (PptxShapeBounds & {
      kind: "shape";
      fill: string | null;
      stroke: string | null;
    });

export interface PptxPreviewSlide {
  index: number;
  widthEmu: number;
  heightEmu: number;
  background: string;
  shapes: PptxPreviewShape[];
}

export interface Settings {
  ocrLangs: string;
  ocrEngine: "tesseract" | "paddle" | "auto";
  pdfOcr: boolean;
  pdfOcrImages: boolean;
  audioLang: string;
  audioThreads: number;
  audioNoSpeechThreshold: number;
  whisperModel: string | null;
  llmEnabled: boolean;
  llmProvider: string;
  llmBaseUrl: string;
  llmModel: string;
  llmApiKey: string | null;
  llmCliBinary: string | null;
  embeddingEnabled: boolean;
  embeddingProvider: string;
  embeddingBaseUrl: string;
  embeddingModel: string;
  embeddingApiKey: string | null;
  embeddingDimensions: number | null;
  embeddingFallbackLocal: boolean;
  autoCheckUpdate: boolean;
}

export type LlmProtocol =
  | "open_ai"
  | "anthropic"
  | "gemini"
  | "open_ai_compatible"
  | "cursor_cli"
  | "codex_cli";

export interface LlmProviderPreset {
  id: string;
  label: string;
  provider: LlmProtocol;
  baseUrl: string | null;
  defaultModel: string;
  models: string[];
  local: boolean;
  requiresApiKey: boolean;
  subscription: boolean;
  supportsVision: boolean;
  supportsEmbeddings: boolean;
  description: string;
}

export interface CliSubscriptionStatus {
  bridge: string;
  authenticated: boolean;
  accountHint: string | null;
  message: string;
}

export interface EmbeddingProviderPreset {
  id: string;
  label: string;
  provider: LlmProtocol;
  baseUrl: string | null;
  defaultModel: string;
  models: string[];
  local: boolean;
  requiresApiKey: boolean;
  defaultDimensions: number | null;
  description: string;
}

export interface EmbeddingConnectionResult {
  provider: string;
  model: string;
  dimensions: number;
  local: boolean;
  latencyMs: number;
}

export interface LlmConnectionResult {
  provider: string;
  model: string;
  baseUrl: string | null;
  local: boolean;
  latencyMs: number;
  response: string;
}

export interface Project {
  id: string;
  name: string;
  rootRel: string;
  createdAt: number;
  importedFrom: string | null;
  implicit: boolean;
}

export interface ImportFolderResult {
  project: Project;
  imported: number;
  skipped: number;
  bytes: number;
  convertRels: string[];
}

export interface KnowledgeIndexStats {
  documents: number;
  chunks: number;
  databaseBytes: number;
  vectorDimensions: number;
  embeddingMode: string;
  embeddingProvider: string;
  embeddingModel: string;
  annAvailable: boolean;
  annThreshold: number;
}

export interface IndexBuildResult {
  documents: number;
  chunks: number;
  indexed: number;
  skipped: number;
  embeddingMode: string;
  embeddingProvider: string;
  embeddingModel: string;
  vectorDimensions: number;
  warnings: string[];
}

export interface SourceAnchor {
  page: number | null;
  slide: number | null;
  sheet: string | null;
  start: number;
  end: number;
}

export interface HybridSearchHit {
  chunkId: string;
  sourceRel: string;
  mdRel: string;
  heading: string;
  snippet: string;
  lexicalScore: number;
  vectorScore: number;
  rerankScore: number;
  anchor: SourceAnchor;
}

export interface HybridSearchResponse {
  hits: HybridSearchHit[];
  warnings: string[];
  embeddingMode: string;
}

export interface GroundedAnswer {
  answer: string;
  citations: HybridSearchHit[];
  mode:
    | "offline_extractive"
    | "local_llm"
    | "cloud_llm"
    | "subscription_cli"
    | "fallback_extractive";
  grounded: boolean;
  warnings: string[];
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

export interface WatchStatus {
  state: "idle" | "watching" | "error";
  rules: number;
  paths: number;
  lastError: string | null;
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
