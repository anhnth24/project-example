import {
  useDeferredValue,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  Archive,
  BookOpenCheck,
  Bot,
  CheckCircle2,
  Clock3,
  Database,
  Download,
  Eye,
  FileSearch,
  GitCompareArrows,
  Play,
  Plus,
  Save,
  Search,
  ShieldCheck,
  Sparkles,
  Table2,
  WandSparkles,
} from "lucide-react";
import { api } from "../lib/ipc";
import { fileIcon } from "../lib/icons";
import { flattenFiles, folderLabel } from "../lib/tree";
import type {
  AskResult,
  DiffHunk,
  DocumentSchema,
  FsNode,
  HandoffMode,
  HandoffResult,
  IntelligenceMode,
  MarkdownTable,
  PiiReport,
  QualityReport,
  SearchHit,
  VersionMeta,
  WatchMatch,
  WatchRule,
} from "../lib/types";
import { useStore } from "../state/store";
import { Button, Notice, Toggle } from "./ui";

const MODES: {
  id: IntelligenceMode;
  label: string;
  icon: ReactNode;
}[] = [
  { id: "handoff", label: "Bàn giao", icon: <BookOpenCheck size={14} /> },
  { id: "quality", label: "Chất lượng", icon: <CheckCircle2 size={14} /> },
  { id: "ask", label: "Hỏi đáp", icon: <Bot size={14} /> },
  { id: "versions", label: "Phiên bản", icon: <Clock3 size={14} /> },
  { id: "tables", label: "Bảng", icon: <Table2 size={14} /> },
  { id: "privacy", label: "PII", icon: <ShieldCheck size={14} /> },
  { id: "export", label: "Xuất", icon: <Archive size={14} /> },
  { id: "watch", label: "Theo dõi", icon: <Eye size={14} /> },
];

function slugify(value: string): string {
  return value
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/đ/gi, "d")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 60);
}

function converted(node: FsNode): boolean {
  return !!node.mdRelPath || node.standaloneMd;
}

export function IntelligenceView() {
  const tree = useStore((state) => state.tree);
  const openNode = useStore((state) => state.openNode);
  const enqueueConversions = useStore((state) => state.enqueueConversions);
  const importSources = useStore((state) => state.importSources);
  const setError = useStore((state) => state.setError);

  const files = useMemo(() => flattenFiles(tree).filter(converted), [tree]);
  const [mode, setMode] = useState<IntelligenceMode>("handoff");
  const [selected, setSelected] = useState<string[]>([]);
  const [productName, setProductName] = useState("Dự án mới");
  const [handoffMode, setHandoffMode] = useState<HandoffMode>("deterministic");
  const [busy, setBusy] = useState<string | null>(null);

  const [handoff, setHandoff] = useState<HandoffResult | null>(null);
  const [activeArtifact, setActiveArtifact] = useState("01-BRD.md");
  const [artifactDrafts, setArtifactDrafts] = useState<Record<string, string>>({});
  const [quality, setQuality] = useState<QualityReport | null>(null);
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [question, setQuestion] = useState("");
  const [useLlm, setUseLlm] = useState(false);
  const [answer, setAnswer] = useState<AskResult | null>(null);
  const [pii, setPii] = useState<PiiReport | null>(null);
  const [redactedPath, setRedactedPath] = useState<string | null>(null);
  const [schemas, setSchemas] = useState<DocumentSchema[]>([]);
  const [tables, setTables] = useState<MarkdownTable[]>([]);
  const [activeTable, setActiveTable] = useState<MarkdownTable | null>(null);
  const [tableRows, setTableRows] = useState<string[][]>([]);
  const [versions, setVersions] = useState<VersionMeta[]>([]);
  const [versionSelection, setVersionSelection] = useState<string[]>([]);
  const [diff, setDiff] = useState<DiffHunk[]>([]);
  const [watchRules, setWatchRulesState] = useState<WatchRule[]>([]);
  const [watchMatches, setWatchMatches] = useState<WatchMatch[]>([]);

  useEffect(() => {
    setSelected((current) => {
      const valid = current.filter((relPath) =>
        files.some((file) => file.relPath === relPath),
      );
      if (valid.length || !files.length) return valid;
      return files.map((file) => file.relPath);
    });
  }, [files]);

  const selectedFiles = files.filter((file) => selected.includes(file.relPath));
  const firstSelected = selectedFiles[0] ?? null;

  function ensureSelection(): boolean {
    if (selected.length) return true;
    setError("Hãy chọn ít nhất một tài liệu đã convert.");
    return false;
  }

  async function run<T>(key: string, task: () => Promise<T>): Promise<T | null> {
    setBusy(key);
    try {
      return await task();
    } catch (error) {
      setError(String(error));
      return null;
    } finally {
      setBusy(null);
    }
  }

  function toggleDocument(relPath: string) {
    setSelected((current) =>
      current.includes(relPath)
        ? current.filter((item) => item !== relPath)
        : [...current, relPath],
    );
  }

  async function generateHandoff() {
    if (!ensureSelection()) return;
    const result = await run("handoff", () =>
      api.generateHandoffPack({
        sourceRels: selected,
        productName: productName.trim() || "Dự án mới",
        productSlug: slugify(productName) || "du-an",
        mode: handoffMode,
      }),
    );
    if (!result) return;
    setHandoff(result);
    setArtifactDrafts(result.pack.artifacts);
    setActiveArtifact(
      result.pack.artifacts["01-BRD.md"]
        ? "01-BRD.md"
        : Object.keys(result.pack.artifacts)[0],
    );
  }

  async function saveArtifact() {
    if (!handoff || !activeArtifact) return;
    const relPath = `${handoff.outRelDir}/${activeArtifact}`;
    await run("save-artifact", () =>
      api.writeTextFile(relPath, artifactDrafts[activeArtifact] ?? ""),
    );
  }

  async function loadQuality() {
    if (!ensureSelection()) return;
    const report = await run("quality", () => api.runQualityReport(selected));
    if (report) setQuality(report);
  }

  async function searchContent() {
    if (!ensureSelection() || deferredQuery.trim().length < 2) return;
    const result = await run("search", () =>
      api.searchIntelligence(selected, deferredQuery, 30),
    );
    if (result) setHits(result);
  }

  async function ask() {
    if (!ensureSelection() || !question.trim()) return;
    const result = await run("ask", () =>
      api.askIntelligence(selected, question, 6, useLlm),
    );
    if (result) setAnswer(result);
  }

  async function scanPii() {
    if (!ensureSelection()) return;
    const result = await run("pii", () => api.scanPii(selected));
    if (result) setPii(result);
  }

  async function redactFirst() {
    if (!firstSelected) return;
    const result = await run("redact", () => api.redactPii(firstSelected.relPath));
    if (result) {
      setPii(result.report);
      setRedactedPath(result.redactedRelPath);
    }
  }

  async function loadSchemas() {
    if (!ensureSelection()) return;
    const result = await run("schema", () => api.extractDocumentSchema(selected));
    if (result) setSchemas(result);
  }

  async function loadTables() {
    if (!firstSelected) return;
    const result = await run("tables", () =>
      api.listMarkdownTables(firstSelected.relPath),
    );
    if (!result) return;
    setTables(result);
    setActiveTable(result[0] ?? null);
    setTableRows(result[0]?.rows.map((row) => [...row]) ?? []);
  }

  function chooseTable(table: MarkdownTable) {
    setActiveTable(table);
    setTableRows(table.rows.map((row) => [...row]));
  }

  async function saveTable() {
    if (!firstSelected || !activeTable) return;
    const result = await run("table-save", () =>
      api.updateMarkdownTable(firstSelected.relPath, activeTable.id, tableRows),
    );
    if (result) {
      await useStore.getState().refreshTree();
      await useStore.getState().loadSession(firstSelected.relPath, true);
      await loadTables();
    }
  }

  async function exportTable() {
    if (!firstSelected || !activeTable) return;
    const output = await saveDialog({
      title: "Xuất bảng CSV",
      defaultPath: `table-${activeTable.index + 1}.csv`,
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    if (!output) return;
    await run("table-export", () =>
      api.exportMarkdownTable(firstSelected.relPath, activeTable.id, output),
    );
  }

  async function loadVersions() {
    if (!firstSelected) return;
    const result = await run("versions", () =>
      api.listDocumentVersions(firstSelected.relPath),
    );
    if (result) setVersions(result);
  }

  async function snapshotVersion() {
    if (!firstSelected) return;
    await run("snapshot", () => api.snapshotDocumentVersion(firstSelected.relPath));
    await loadVersions();
  }

  async function compareVersions() {
    if (!firstSelected || versionSelection.length !== 2) return;
    const result = await run("diff", () =>
      api.diffDocumentVersions(
        firstSelected.relPath,
        versionSelection[1],
        versionSelection[0],
      ),
    );
    if (result) setDiff(result);
  }

  function toggleVersion(id: string) {
    setVersionSelection((current) => {
      if (current.includes(id)) return current.filter((item) => item !== id);
      return [id, ...current].slice(0, 2);
    });
  }

  async function loadWatchRules() {
    const result = await run("watch-load", api.getWatchRules);
    if (result) setWatchRulesState(result);
  }

  async function addWatchRule() {
    const directory = await openDialog({
      directory: true,
      multiple: false,
      title: "Chọn thư mục cần theo dõi",
    });
    if (!directory || Array.isArray(directory)) return;
    const next: WatchRule = {
      id: `watch-${Date.now()}`,
      watchAbs: directory,
      targetFolderRel: "",
      pattern: "*.pdf",
      action: "import_and_convert",
      enabled: true,
    };
    const rules = [...watchRules, next];
    const saved = await run("watch-save", () => api.setWatchRules(rules));
    if (saved === null) return;
    setWatchRulesState(rules);
  }

  async function scanWatch() {
    const result = await run("watch-scan", api.scanWatchRules);
    if (result) setWatchMatches(result);
  }

  async function importWatchMatches() {
    const paths = watchMatches.map((match) => match.sourceAbs);
    if (!paths.length) return;
    await importSources(paths);
    setWatchMatches([]);
  }

  async function exportPack() {
    if (!ensureSelection()) return;
    const output = await saveDialog({
      title: "Xuất Knowledge Pack",
      defaultPath: `${slugify(productName) || "knowledge-pack"}.zip`,
      filters: [{ name: "ZIP", extensions: ["zip"] }],
    });
    if (!output) return;
    await run("export", () =>
      api.exportKnowledgePack({
        sourceRels: selected,
        productName: productName.trim() || "Dự án mới",
        productSlug: slugify(productName) || "du-an",
        outputAbs: output,
      }),
    );
  }

  function openSource(relPath: string) {
    const node = files.find((file) => file.relPath === relPath);
    if (node) openNode(node);
  }

  return (
    <section className="intelligence-view">
      <header className="intelligence-header">
        <div>
          <span className="eyebrow">Document intelligence</span>
          <h1>Bàn giao BA/PM</h1>
          <p>Sinh BRD/PRD có trích dẫn, kiểm tra rồi đóng gói cho Dev.</p>
        </div>
        <div className="scope-summary">
          <Database size={15} />
          <span>
            <b>{selected.length}</b>/{files.length} tài liệu
          </span>
        </div>
      </header>

      <nav className="intelligence-nav" aria-label="Công cụ intelligence">
        {MODES.map((item) => (
          <button
            type="button"
            key={item.id}
            className={mode === item.id ? "active" : ""}
            aria-pressed={mode === item.id}
            onClick={() => setMode(item.id)}
          >
            {item.icon}
            {item.label}
          </button>
        ))}
      </nav>

      <div className="intelligence-layout">
        <aside className="corpus-panel">
          <header>
            <strong>Corpus nguồn</strong>
            <button type="button" onClick={() => setSelected(files.map((file) => file.relPath))}>
              Chọn tất cả
            </button>
          </header>
          <div className="corpus-list">
            {!files.length && (
              <div className="intelligence-empty">
                Hãy convert tài liệu trước khi tạo bàn giao.
              </div>
            )}
            {files.map((file) => (
              <label key={file.relPath} className="corpus-item">
                <input
                  type="checkbox"
                  checked={selected.includes(file.relPath)}
                  onChange={() => toggleDocument(file.relPath)}
                />
                <span className="corpus-file-icon">{fileIcon(file, { size: 15 })}</span>
                <span>
                  <b>{file.name}</b>
                  <small>{folderLabel(file.relPath)}</small>
                </span>
              </label>
            ))}
          </div>
        </aside>

        <main className="intelligence-main">
          {mode === "handoff" && (
            <div className="intelligence-panel handoff-studio">
              <div className="panel-title">
                <div>
                  <span className="eyebrow">Handoff studio</span>
                  <h2>Sinh BRD/PRD và bộ bàn giao</h2>
                </div>
                <Button
                  variant="primary"
                  icon={<WandSparkles size={15} />}
                  loading={busy === "handoff"}
                  onClick={generateHandoff}
                >
                  Sinh bộ bàn giao
                </Button>
              </div>

              <div className="handoff-config">
                <label className="field">
                  <span>Tên sản phẩm / dự án</span>
                  <input
                    value={productName}
                    onChange={(event) => setProductName(event.target.value)}
                  />
                </label>
                <Toggle
                  checked={handoffMode === "llm_assisted"}
                  onChange={(checked) =>
                    setHandoffMode(checked ? "llm_assisted" : "deterministic")
                  }
                  label="Tăng cường bằng LLM"
                  description="Tắt mặc định. Không có FILECONV_LLM_* sẽ giữ bản offline."
                />
              </div>

              {!handoff ? (
                <div className="handoff-placeholder">
                  <Sparkles size={30} />
                  <strong>8 tài liệu bàn giao từ một lần sinh</strong>
                  <span>
                    BRD, PRD, user stories, acceptance criteria, glossary, test cases,
                    traceability và câu hỏi mở.
                  </span>
                </div>
              ) : (
                <>
                  {handoff.llmNote && <Notice tone="info">{handoff.llmNote}</Notice>}
                  <div className="handoff-validation">
                    <span className={handoff.pack.validation.ok ? "ok" : "bad"}>
                      {handoff.pack.validation.ok ? "Validation đạt" : "Cần rà soát"}
                    </span>
                    <span>
                      Citation {(handoff.pack.validation.citationCoverage * 100).toFixed(0)}%
                    </span>
                    <span>{handoff.pack.validation.warnings.length} cảnh báo</span>
                    <span>{handoff.pack.items.length} mục</span>
                  </div>
                  <div className="artifact-tabs">
                    {Object.keys(handoff.pack.artifacts).map((name) => (
                      <button
                        type="button"
                        className={activeArtifact === name ? "active" : ""}
                        key={name}
                        onClick={() => setActiveArtifact(name)}
                      >
                        {name.replace(/^\d+-/, "").replace(".md", "")}
                      </button>
                    ))}
                  </div>
                  <textarea
                    className="artifact-editor"
                    aria-label={`Nội dung ${activeArtifact}`}
                    value={artifactDrafts[activeArtifact] ?? ""}
                    onChange={(event) =>
                      setArtifactDrafts((current) => ({
                        ...current,
                        [activeArtifact]: event.target.value,
                      }))
                    }
                  />
                  <div className="panel-actions">
                    <span>Đã lưu tại {handoff.outRelDir}</span>
                    <Button
                      variant="primary"
                      size="sm"
                      icon={<Save size={13} />}
                      loading={busy === "save-artifact"}
                      onClick={saveArtifact}
                    >
                      Lưu artifact
                    </Button>
                  </div>
                </>
              )}
            </div>
          )}

          {mode === "quality" && (
            <div className="intelligence-panel">
              <PanelHeading
                eyebrow="Quality gate"
                title="Chất lượng corpus"
                action={
                  <Button
                    variant="primary"
                    icon={<Play size={14} />}
                    loading={busy === "quality"}
                    onClick={loadQuality}
                  >
                    Phân tích
                  </Button>
                }
              />
              {!quality ? (
                <EmptyTool text="Chạy phân tích để xem lỗi OCR, encoding, bảng và nội dung thiếu." />
              ) : (
                <>
                  <div className="quality-score">
                    <b>{Math.round(quality.score * 100)}</b>
                    <span>điểm chất lượng · {quality.issueCount} vấn đề</span>
                  </div>
                  <div className="quality-docs">
                    {quality.documents.map((document) => (
                      <article key={document.sourceRel}>
                        <header>
                          <strong>{document.sourceRel}</strong>
                          <span>{Math.round(document.score * 100)}%</span>
                        </header>
                        <small>
                          {document.chars.toLocaleString("vi-VN")} ký tự · {document.headings} heading
                          · {document.tables} bảng
                        </small>
                        {document.issues.map((issue) => (
                          <div className={`quality-issue ${issue.severity}`} key={issue.code}>
                            <b>{issue.message}</b>
                            <span>{issue.recommendation}</span>
                          </div>
                        ))}
                        <div className="inline-actions">
                          <Button variant="ghost" size="sm" onClick={() => openSource(document.sourceRel)}>
                            Mở đối chiếu
                          </Button>
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => enqueueConversions([document.sourceRel])}
                          >
                            Reprocess
                          </Button>
                          {files.find((file) => file.relPath === document.sourceRel)?.kind ===
                            "image" && (
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() =>
                                void run("hard-ocr", () =>
                                  api.hardOcrImage(document.sourceRel),
                                )
                              }
                            >
                              OCR hard
                            </Button>
                          )}
                        </div>
                      </article>
                    ))}
                  </div>
                </>
              )}
            </div>
          )}

          {mode === "ask" && (
            <div className="intelligence-panel">
              <PanelHeading eyebrow="Cited intelligence" title="Tìm kiếm và hỏi đáp" />
              <div className="search-row">
                <Search size={16} />
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  onKeyDown={(event) => event.key === "Enter" && void searchContent()}
                  placeholder="Tìm nội dung trong corpus…"
                  aria-label="Tìm nội dung corpus"
                />
                <Button loading={busy === "search"} onClick={searchContent}>
                  Tìm
                </Button>
              </div>
              {!!hits.length && (
                <div className="search-hits">
                  {hits.map((hit) => (
                    <button
                      type="button"
                      key={hit.chunk.id}
                      onClick={() => openSource(hit.chunk.sourceRel)}
                    >
                      <b>{hit.chunk.heading || hit.chunk.sourceRel}</b>
                      <span>{hit.snippet}</span>
                      <small>
                        {hit.chunk.sourceRel}
                        {hit.chunk.page ? ` · trang ${hit.chunk.page}` : ""}
                      </small>
                    </button>
                  ))}
                </div>
              )}
              <div className="ask-box">
                <textarea
                  value={question}
                  onChange={(event) => setQuestion(event.target.value)}
                  placeholder="Đặt câu hỏi chỉ dựa trên tài liệu đã chọn…"
                />
                <div>
                  <Toggle
                    checked={useLlm}
                    onChange={setUseLlm}
                    label="LLM"
                    description="Tắt = trả lời trích xuất offline."
                  />
                  <Button
                    variant="primary"
                    icon={<Bot size={14} />}
                    loading={busy === "ask"}
                    onClick={ask}
                  >
                    Hỏi corpus
                  </Button>
                </div>
              </div>
              {answer && (
                <div className="answer-pane">
                  <pre>{answer.answer}</pre>
                  <div className="citation-list">
                    {answer.citations.map((citation) => (
                      <button
                        type="button"
                        key={citation.id}
                        onClick={() => openSource(citation.sourceRel)}
                      >
                        <b>[{citation.id}] {citation.heading || citation.sourceRel}</b>
                        <span>{citation.quote}</span>
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}

          {mode === "versions" && (
            <div className="intelligence-panel">
              <PanelHeading
                eyebrow="Version safety"
                title="Phiên bản và diff"
                action={
                  <div className="inline-actions">
                    <Button loading={busy === "versions"} onClick={loadVersions}>
                      Tải danh sách
                    </Button>
                    <Button
                      variant="primary"
                      icon={<Plus size={13} />}
                      loading={busy === "snapshot"}
                      onClick={snapshotVersion}
                    >
                      Snapshot
                    </Button>
                  </div>
                }
              />
              {!firstSelected ? (
                <EmptyTool text="Chọn một tài liệu để quản lý phiên bản." />
              ) : (
                <>
                  <div className="version-list">
                    {versions.map((version) => (
                      <label key={version.id}>
                        <input
                          type="checkbox"
                          checked={versionSelection.includes(version.id)}
                          onChange={() => toggleVersion(version.id)}
                        />
                        <span>
                          <b>{version.id}</b>
                          <small>
                            {new Date(version.createdAt * 1000).toLocaleString("vi-VN")} ·{" "}
                            {version.bytes.toLocaleString("vi-VN")} B
                          </small>
                        </span>
                      </label>
                    ))}
                  </div>
                  <Button
                    icon={<GitCompareArrows size={14} />}
                    disabled={versionSelection.length !== 2}
                    loading={busy === "diff"}
                    onClick={compareVersions}
                  >
                    So sánh 2 phiên bản
                  </Button>
                  {diff.map((hunk, index) => (
                    <div className={`diff-hunk ${hunk.kind}`} key={`${hunk.kind}-${index}`}>
                      <strong>{hunk.kind}</strong>
                      <div>
                        <pre>{hunk.oldText}</pre>
                        <pre>{hunk.newText}</pre>
                      </div>
                    </div>
                  ))}
                </>
              )}
            </div>
          )}

          {mode === "tables" && (
            <div className="intelligence-panel">
              <PanelHeading
                eyebrow="Structured data"
                title="Bảng và schema"
                action={
                  <div className="inline-actions">
                    <Button loading={busy === "schema"} onClick={loadSchemas}>
                      Trích schema
                    </Button>
                    <Button
                      variant="primary"
                      loading={busy === "tables"}
                      onClick={loadTables}
                    >
                      Mở bảng
                    </Button>
                  </div>
                }
              />
              {!!schemas.length && (
                <div className="schema-grid">
                  {schemas.map((schema) => (
                    <article key={schema.sourceRel}>
                      <strong>{schema.sourceRel}</strong>
                      <span>{schema.headings.length} heading · {schema.fields.length} field</span>
                      <div>
                        {schema.fields.slice(0, 12).map((field) => (
                          <code key={`${schema.sourceRel}-${field.name}`}>
                            {field.name}: {field.fieldType}
                          </code>
                        ))}
                      </div>
                    </article>
                  ))}
                </div>
              )}
              <div className="table-workspace">
                <aside>
                  {tables.map((table) => (
                    <button
                      type="button"
                      key={table.id}
                      className={activeTable?.id === table.id ? "active" : ""}
                      onClick={() => chooseTable(table)}
                    >
                      Bảng {table.index + 1} · {table.rows.length} dòng
                    </button>
                  ))}
                </aside>
                {activeTable ? (
                  <div className="editable-table-wrap">
                    <table className="editable-table">
                      <tbody>
                        {tableRows.map((row, rowIndex) => (
                          <tr key={`${activeTable.id}-${rowIndex}`}>
                            {row.map((cell, columnIndex) => (
                              <td key={`${rowIndex}-${columnIndex}`}>
                                <input
                                  value={cell}
                                  onChange={(event) =>
                                    setTableRows((current) =>
                                      current.map((currentRow, currentRowIndex) =>
                                        currentRowIndex === rowIndex
                                          ? currentRow.map((currentCell, currentColumnIndex) =>
                                              currentColumnIndex === columnIndex
                                                ? event.target.value
                                                : currentCell,
                                            )
                                          : currentRow,
                                      ),
                                    )
                                  }
                                />
                              </td>
                            ))}
                          </tr>
                        ))}
                      </tbody>
                    </table>
                    <Button
                      variant="primary"
                      icon={<Save size={13} />}
                      loading={busy === "table-save"}
                      onClick={saveTable}
                    >
                      Lưu vào Markdown
                    </Button>
                    <Button
                      variant="ghost"
                      icon={<Download size={13} />}
                      loading={busy === "table-export"}
                      onClick={exportTable}
                    >
                      Xuất CSV
                    </Button>
                  </div>
                ) : (
                  <EmptyTool text="Chưa chọn bảng." />
                )}
              </div>
            </div>
          )}

          {mode === "privacy" && (
            <div className="intelligence-panel">
              <PanelHeading
                eyebrow="Privacy"
                title="PII và bản chia sẻ"
                action={
                  <Button
                    variant="primary"
                    icon={<ShieldCheck size={14} />}
                    loading={busy === "pii"}
                    onClick={scanPii}
                  >
                    Quét cục bộ
                  </Button>
                }
              />
              <Notice tone="info">
                Quét regex chạy hoàn toàn trên máy; chỉ dùng LLM khi bạn bật ở Hỏi đáp.
              </Notice>
              {!pii ? (
                <EmptyTool text="Quét để tìm email, số điện thoại, CCCD/CMND và tài khoản." />
              ) : (
                <>
                  <div className="pii-summary">
                    <b>{pii.findings.length}</b>
                    <span>phát hiện PII</span>
                  </div>
                  <div className="pii-list">
                    {pii.findings.map((finding, index) => (
                      <div key={`${finding.sourceRel}-${finding.start}-${index}`}>
                        <code>{finding.kind}</code>
                        <b>{finding.text}</b>
                        <span>{finding.sourceRel}</span>
                      </div>
                    ))}
                  </div>
                  <Button
                    variant="primary"
                    loading={busy === "redact"}
                    disabled={!firstSelected}
                    onClick={redactFirst}
                  >
                    Tạo bản đã che của tài liệu đầu tiên
                  </Button>
                  {redactedPath && <small>Đã ghi: {redactedPath}</small>}
                </>
              )}
            </div>
          )}

          {mode === "export" && (
            <div className="intelligence-panel export-panel">
              <PanelHeading eyebrow="Delivery" title="Knowledge Pack" />
              <Archive size={42} />
              <h3>Đóng gói tài liệu để bàn giao</h3>
              <p>
                ZIP gồm BRD/PRD, citations, validation, Markdown nguồn, user stories,
                acceptance criteria, test cases và traceability.
              </p>
              <Button
                variant="primary"
                icon={<Download size={15} />}
                loading={busy === "export"}
                onClick={exportPack}
              >
                Xuất Knowledge Pack
              </Button>
            </div>
          )}

          {mode === "watch" && (
            <div className="intelligence-panel">
              <PanelHeading
                eyebrow="Automation"
                title="Watch folders"
                action={
                  <div className="inline-actions">
                    <Button onClick={loadWatchRules}>Tải rules</Button>
                    <Button variant="primary" icon={<Plus size={13} />} onClick={addWatchRule}>
                      Thêm rule
                    </Button>
                  </div>
                }
              />
              <div className="watch-rules">
                {watchRules.map((rule) => (
                  <article key={rule.id}>
                    <strong>{rule.watchAbs}</strong>
                    <span>{rule.pattern} → {rule.targetFolderRel || "DATA"}</span>
                    <code>{rule.action}</code>
                  </article>
                ))}
              </div>
              <div className="inline-actions">
                <Button
                  icon={<FileSearch size={14} />}
                  loading={busy === "watch-scan"}
                  onClick={scanWatch}
                >
                  Quét thay đổi
                </Button>
                <Button
                  variant="primary"
                  disabled={!watchMatches.length}
                  onClick={importWatchMatches}
                >
                  Import {watchMatches.length || ""}
                </Button>
              </div>
              {watchMatches.map((match) => (
                <div className="watch-match" key={`${match.ruleId}-${match.sourceAbs}`}>
                  {match.sourceAbs}
                </div>
              ))}
            </div>
          )}
        </main>
      </div>
    </section>
  );
}

function PanelHeading({
  eyebrow,
  title,
  action,
}: {
  eyebrow: string;
  title: string;
  action?: ReactNode;
}) {
  return (
    <div className="panel-title">
      <div>
        <span className="eyebrow">{eyebrow}</span>
        <h2>{title}</h2>
      </div>
      {action}
    </div>
  );
}

function EmptyTool({ text }: { text: string }) {
  return (
    <div className="intelligence-empty tool-empty">
      <Sparkles size={26} />
      <span>{text}</span>
    </div>
  );
}
