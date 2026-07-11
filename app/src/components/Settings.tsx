import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  Cloud,
  BrainCircuit,
  FolderOpen,
  LogIn,
  RotateCcw,
  Server,
  SquareTerminal,
  Wifi,
} from "lucide-react";
import { api } from "../lib/ipc";
import {
  applyEmbeddingPreset,
  applyLlmPreset,
  validateEmbeddingSettings,
  validateLlmSettings,
} from "../lib/llmSettings";
import { useStore } from "../state/store";
import type {
  LlmConnectionResult,
  LlmProviderPreset,
  CliSubscriptionStatus,
  EmbeddingConnectionResult,
  EmbeddingProviderPreset,
  Settings,
} from "../lib/types";
import {
  Button,
  Combobox,
  Modal,
  Notice,
  SelectControl,
  Toggle,
} from "./ui";

const DEFAULTS: Settings = {
  ocrLangs: "vie+eng",
  ocrEngine: "tesseract",
  pdfOcr: true,
  pdfOcrImages: false,
  audioLang: "vi",
  audioThreads: 4,
  audioNoSpeechThreshold: 0.6,
  whisperModel: null,
  llmEnabled: false,
  llmProvider: "ollama",
  llmBaseUrl: "http://127.0.0.1:11434",
  llmModel: "qwen2.5:7b",
  llmApiKey: null,
  llmCliBinary: null,
  embeddingEnabled: false,
  embeddingProvider: "ollama",
  embeddingBaseUrl: "http://127.0.0.1:11434",
  embeddingModel: "nomic-embed-text",
  embeddingApiKey: null,
  embeddingDimensions: null,
  embeddingFallbackLocal: true,
};

function providerOptionLabel(preset: LlmProviderPreset): string {
  const cleanLabel = preset.label.replace(/\s*\((?:Local|Self-host)\)$/i, "");
  const kind = preset.subscription ? "Subscription" : preset.local ? "Local" : "Cloud";
  return `${kind} · ${cleanLabel}`;
}

export function SettingsModal({ onClose }: { onClose: () => void }) {
  const current = useStore((s) => s.settings) ?? DEFAULTS;
  const saveSettings = useStore((s) => s.saveSettings);
  const setError = useStore((s) => s.setError);
  const [form, setForm] = useState<Settings>(current);
  const [saving, setSaving] = useState(false);
  const [presets, setPresets] = useState<LlmProviderPreset[]>([]);
  const [testingLlm, setTestingLlm] = useState(false);
  const [llmResult, setLlmResult] = useState<LlmConnectionResult | null>(null);
  const [cliStatus, setCliStatus] = useState<CliSubscriptionStatus | null>(null);
  const [checkingCli, setCheckingCli] = useState(false);
  const [embeddingPresets, setEmbeddingPresets] = useState<
    EmbeddingProviderPreset[]
  >([]);
  const [testingEmbedding, setTestingEmbedding] = useState(false);
  const [embeddingResult, setEmbeddingResult] =
    useState<EmbeddingConnectionResult | null>(null);

  useEffect(() => {
    api
      .getLlmProviderPresets()
      .then(setPresets)
      .catch((error) => setError(String(error)));
    api
      .getEmbeddingProviderPresets()
      .then(setEmbeddingPresets)
      .catch((error) => setError(String(error)));
  }, [setError]);

  function set<K extends keyof Settings>(key: K, val: Settings[K]) {
    setForm((f) => ({ ...f, [key]: val }));
  }

  async function pickWhisper() {
    try {
      const picked = await openDialog({
        multiple: false,
        title: "Chọn model whisper (.bin GGML)",
        filters: [{ name: "GGML", extensions: ["bin"] }],
      });
      if (picked && !Array.isArray(picked)) set("whisperModel", picked);
    } catch (e) {
      setError(String(e));
    }
  }

  async function onSave() {
    if (validation.length) return;
    setSaving(true);
    try {
      await saveSettings(form);
      onClose();
    } catch {
      // Store already exposes the backend error through the global alert.
    } finally {
      setSaving(false);
    }
  }

  const selectedPreset = presets.find((preset) => preset.id === form.llmProvider);
  const selectedEmbeddingPreset = embeddingPresets.find(
    (preset) => preset.id === form.embeddingProvider,
  );

  function applyPreset(id: string) {
    const preset = presets.find((item) => item.id === id);
    if (!preset) {
      set("llmProvider", id);
      return;
    }
    setForm((currentForm) => applyLlmPreset(currentForm, preset));
    setLlmResult(null);
    setCliStatus(null);
  }

  async function checkCliSubscription() {
    if (validation.length) return;
    setCheckingCli(true);
    try {
      await saveSettings(form);
      setCliStatus(await api.getCliSubscriptionStatus());
    } catch {
      setCliStatus(null);
    } finally {
      setCheckingCli(false);
    }
  }

  async function startCliLogin() {
    if (validation.length) return;
    setCheckingCli(true);
    try {
      await saveSettings(form);
      await api.startCliSubscriptionLogin();
      setCliStatus({
        bridge: selectedPreset?.label ?? "CLI",
        authenticated: false,
        accountHint: null,
        message: "Đã mở luồng đăng nhập chính thức. Hoàn tất trên trình duyệt rồi kiểm tra lại.",
      });
    } finally {
      setCheckingCli(false);
    }
  }

  async function testLlmConnection() {
    if (validation.length) return;
    setTestingLlm(true);
    setLlmResult(null);
    try {
      await saveSettings(form);
      setLlmResult(await api.testLlmConnection());
    } catch {
      // Store/global alert already contains the actionable error.
    } finally {
      setTestingLlm(false);
    }
  }

  function applyEmbedding(id: string) {
    const preset = embeddingPresets.find((item) => item.id === id);
    if (!preset) {
      set("embeddingProvider", id);
      return;
    }
    setForm((currentForm) => applyEmbeddingPreset(currentForm, preset));
    setEmbeddingResult(null);
  }

  async function testEmbeddingConnection() {
    if (validation.length) return;
    setTestingEmbedding(true);
    setEmbeddingResult(null);
    try {
      await saveSettings(form);
      setEmbeddingResult(await api.testEmbeddingConnection());
    } finally {
      setTestingEmbedding(false);
    }
  }

  const validation: string[] = [];
  if (!/^[a-z]{3}(?:\+[a-z]{3})*$/i.test(form.ocrLangs.trim())) {
    validation.push("Ngôn ngữ OCR cần có dạng vie hoặc vie+eng.");
  }
  if (!form.audioLang.trim()) validation.push("Ngôn ngữ audio không được để trống.");
  if (
    !Number.isFinite(form.audioThreads) ||
    form.audioThreads < 1 ||
    form.audioThreads > 32
  ) {
    validation.push("Thread audio phải nằm trong khoảng 1–32.");
  }
  if (
    !Number.isFinite(form.audioNoSpeechThreshold) ||
    form.audioNoSpeechThreshold < 0 ||
    form.audioNoSpeechThreshold > 1
  ) {
    validation.push("Ngưỡng no-speech phải nằm trong khoảng 0–1.");
  }
  validation.push(...validateLlmSettings(form, selectedPreset));
  validation.push(
    ...validateEmbeddingSettings(form, selectedEmbeddingPreset),
  );

  return (
    <Modal
      title="Cài đặt Markhand"
      description="Convert chạy local. LLM mặc định ưu tiên self-host; cloud chỉ nhận context khi bạn chủ động bật."
      onClose={onClose}
      width={680}
      footer={
        <>
          <Button
            variant="ghost"
            icon={<RotateCcw size={14} />}
            onClick={() => setForm(DEFAULTS)}
          >
            Khôi phục mặc định
          </Button>
          <span className="modal-footer-spacer" />
          <Button variant="ghost" onClick={onClose}>
            Hủy
          </Button>
          <Button
            variant="primary"
            loading={saving}
            disabled={validation.length > 0}
            onClick={onSave}
          >
            Lưu
          </Button>
        </>
      }
    >
      <div className="settings-form">
        <div className="settings-grid ocr-grid">
          <label className="field">
            <span>Ngôn ngữ OCR (ảnh / PDF scan)</span>
            <input
              value={form.ocrLangs}
              onChange={(event) => set("ocrLangs", event.target.value)}
              placeholder="vie+eng"
            />
            <small>Có thể ghép các mã Tesseract bằng dấu “+”.</small>
          </label>
          <div className="field">
            <span>OCR engine</span>
            <SelectControl
              value={form.ocrEngine}
              onChange={(value) =>
                set("ocrEngine", value as Settings["ocrEngine"])
              }
              ariaLabel="Chọn OCR engine"
              options={[
                { value: "tesseract", label: "Tesseract · mặc định" },
                { value: "auto", label: "Auto · Paddle fallback" },
                { value: "paddle", label: "PaddleOCR · tùy chọn" },
              ]}
            />
          </div>
        </div>

        <Toggle
          checked={form.pdfOcr}
          onChange={(checked) => set("pdfOcr", checked)}
          label="OCR trang PDF dạng scan"
          description="Dùng khi trang có ít hoặc không có lớp text."
        />
        <Toggle
          checked={form.pdfOcrImages}
          onChange={(checked) => set("pdfOcrImages", checked)}
          label="OCR thêm ảnh nhúng"
          description="Chính xác hơn cho trang trộn nhưng thời gian xử lý lâu hơn."
        />

        <div className="settings-grid audio-grid">
          <label className="field">
            <span>Ngôn ngữ audio</span>
            <input
              value={form.audioLang}
              onChange={(event) => set("audioLang", event.target.value)}
            />
          </label>
          <label className="field">
            <span>Thread audio</span>
            <input
              type="number"
              min={1}
              max={32}
              value={form.audioThreads}
              onChange={(event) => set("audioThreads", Number(event.target.value))}
            />
          </label>
          <label className="field">
            <span>Ngưỡng no-speech</span>
            <input
              type="number"
              min={0}
              max={1}
              step={0.05}
              value={form.audioNoSpeechThreshold}
              onChange={(event) =>
                set("audioNoSpeechThreshold", Number(event.target.value))
              }
            />
            <small>Cao hơn = giữ nhiều segment hơn.</small>
          </label>
        </div>

        <div className="field">
          <label htmlFor="whisper-model">Model Whisper (.bin)</label>
          <div className="field-with-action">
            <input
              id="whisper-model"
              value={form.whisperModel ?? ""}
              onChange={(event) => set("whisperModel", event.target.value || null)}
              placeholder="Để trống nếu không dùng audio"
            />
            <Button variant="secondary" icon={<FolderOpen size={14} />} onClick={pickWhisper}>
              Chọn
            </Button>
          </div>
        </div>

        <div className="settings-section-heading">
          <span className="eyebrow">Document Intelligence</span>
          <strong>LLM provider</strong>
        </div>

        <Toggle
          checked={form.llmEnabled}
          onChange={(checked) => set("llmEnabled", checked)}
          label="Bật LLM cho Handoff và Hỏi đáp"
          description="Tắt = search/Q&A extractive và BRD/PRD deterministic hoàn toàn local."
        />

        {form.llmEnabled && (
          <div className="llm-settings">
            <div className="field">
              <span>Provider preset</span>
              <SelectControl
                value={form.llmProvider}
                onChange={applyPreset}
                ariaLabel="Chọn LLM provider"
                options={presets.map((preset) => ({
                  value: preset.id,
                  label: providerOptionLabel(preset),
                }))}
              />
            </div>

            {selectedPreset && (
              <Notice
                tone={
                  selectedPreset.local || selectedPreset.subscription
                    ? "info"
                    : "warning"
                }
              >
                <span className="provider-notice">
                  {selectedPreset.subscription ? (
                    <SquareTerminal size={14} />
                  ) : selectedPreset.local ? (
                    <Server size={14} />
                  ) : (
                    <Cloud size={14} />
                  )}
                  <span>
                    <b>
                      {selectedPreset.subscription
                        ? "Subscription qua official CLI"
                        : selectedPreset.local
                        ? "100% local — khuyến nghị"
                        : "Dữ liệu top citation sẽ rời máy"}
                    </b>
                    <small>{selectedPreset.description}</small>
                  </span>
                </span>
              </Notice>
            )}

            <div
              className={`settings-grid llm-grid ${
                selectedPreset?.subscription ? "subscription-grid" : ""
              }`}
            >
              {!selectedPreset?.subscription && (
                <label className="field">
                  <span>Base URL</span>
                  <input
                    value={form.llmBaseUrl}
                    onChange={(event) => set("llmBaseUrl", event.target.value)}
                    placeholder="http://127.0.0.1:11434"
                  />
                </label>
              )}
              <div className="field">
                <span>Model</span>
                <Combobox
                  value={form.llmModel}
                  onChange={(model) => set("llmModel", model)}
                  options={selectedPreset?.models ?? []}
                  ariaLabel="Model LLM"
                  placeholder="qwen2.5:7b"
                />
              </div>
              {selectedPreset?.subscription && (
                <label className="field">
                  <span>CLI binary override (tùy chọn)</span>
                  <input
                    value={form.llmCliBinary ?? ""}
                    onChange={(event) =>
                      set("llmCliBinary", event.target.value || null)
                    }
                    placeholder={
                      form.llmProvider === "cursor-cli"
                        ? "Tự tìm agent trong PATH"
                        : "Tự tìm codex trong PATH"
                    }
                  />
                </label>
              )}
            </div>

            {!selectedPreset?.subscription && (
              <label className="field">
                <span>
                  API key{" "}
                  {selectedPreset?.requiresApiKey ? "(bắt buộc)" : "(tùy chọn)"}
                </span>
                <input
                  type="password"
                  autoComplete="off"
                  value={form.llmApiKey ?? ""}
                  onChange={(event) =>
                    set("llmApiKey", event.target.value || null)
                  }
                  placeholder={
                    selectedPreset?.local
                      ? "Local provider thường không cần key"
                      : "Chỉ giữ trong memory; dùng FILECONV_LLM_API_KEY để persist"
                  }
                />
                <small>
                  Markhand không ghi API key vào settings.json. Sau khi restart,
                  dùng biến môi trường hoặc nhập lại.
                </small>
              </label>
            )}

            {selectedPreset?.subscription && (
              <div className="subscription-actions">
                <div className="inline-actions">
                  <Button
                    variant="secondary"
                    icon={<LogIn size={14} />}
                    loading={checkingCli}
                    disabled={validation.length > 0}
                    onClick={startCliLogin}
                  >
                    Đăng nhập bằng trình duyệt
                  </Button>
                  <Button
                    variant="ghost"
                    loading={checkingCli}
                    disabled={validation.length > 0}
                    onClick={checkCliSubscription}
                  >
                    Kiểm tra đăng nhập
                  </Button>
                </div>
                {cliStatus && (
                  <small
                    className={
                      cliStatus.authenticated ? "local-result" : "cli-pending"
                    }
                  >
                    {cliStatus.authenticated ? "Đã kết nối" : "Chờ đăng nhập"} ·{" "}
                    {cliStatus.message}
                  </small>
                )}
              </div>
            )}

            {form.llmProvider === "ollama" && (
              <div className="local-setup">
                <code>ollama serve</code>
                <code>ollama pull {form.llmModel || "qwen2.5:7b"}</code>
              </div>
            )}

            <div className="llm-test-row">
              <Button
                variant="secondary"
                icon={<Wifi size={14} />}
                loading={testingLlm}
                disabled={validation.length > 0}
                onClick={testLlmConnection}
              >
                Test kết nối
              </Button>
              {llmResult && (
                <span className={llmResult.local ? "local-result" : ""}>
                  OK · {llmResult.model} · {llmResult.latencyMs}ms ·{" "}
                  {llmResult.response}
                </span>
              )}
            </div>
          </div>
        )}

        <div className="settings-section-heading">
          <span className="eyebrow">Hybrid retrieval</span>
          <strong>Neural embeddings</strong>
        </div>

        <Toggle
          checked={form.embeddingEnabled}
          onChange={(checked) => set("embeddingEnabled", checked)}
          label="Bật semantic embeddings"
          description="Tắt = FTS5 + feature hashing 256D chạy hoàn toàn offline."
        />

        {form.embeddingEnabled && (
          <div className="llm-settings">
            <div className="field">
              <span>Embedding provider</span>
              <SelectControl
                value={form.embeddingProvider}
                onChange={applyEmbedding}
                ariaLabel="Chọn embedding provider"
                options={embeddingPresets.map((preset) => ({
                  value: preset.id,
                  label: `${preset.local ? "Local" : "Cloud"} · ${preset.label.replace(
                    /\s*\((?:Local|Self-host)\)$/i,
                    "",
                  )}`,
                }))}
              />
            </div>

            {selectedEmbeddingPreset && (
              <Notice tone={selectedEmbeddingPreset.local ? "info" : "warning"}>
                <span className="provider-notice">
                  <BrainCircuit size={14} />
                  <span>
                    <b>
                      {selectedEmbeddingPreset.local
                        ? "Neural search chạy local"
                        : "Toàn bộ chunk sẽ được gửi lên cloud khi build index"}
                    </b>
                    <small>{selectedEmbeddingPreset.description}</small>
                  </span>
                </span>
              </Notice>
            )}

            <div className="settings-grid llm-grid">
              <label className="field">
                <span>Base URL</span>
                <input
                  value={form.embeddingBaseUrl}
                  onChange={(event) =>
                    set("embeddingBaseUrl", event.target.value)
                  }
                  placeholder="http://127.0.0.1:11434"
                />
              </label>
              <div className="field">
                <span>Embedding model</span>
                <Combobox
                  value={form.embeddingModel}
                  onChange={(model) => set("embeddingModel", model)}
                  options={selectedEmbeddingPreset?.models ?? []}
                  ariaLabel="Model embedding"
                  placeholder="nomic-embed-text"
                />
              </div>
            </div>

            <div className="settings-grid">
              <label className="field">
                <span>Số chiều (để trống = model mặc định)</span>
                <input
                  type="number"
                  min={32}
                  max={4096}
                  value={form.embeddingDimensions ?? ""}
                  onChange={(event) =>
                    set(
                      "embeddingDimensions",
                      event.target.value ? Number(event.target.value) : null,
                    )
                  }
                  placeholder="768"
                />
              </label>
              <label className="field">
                <span>
                  API key{" "}
                  {selectedEmbeddingPreset?.requiresApiKey
                    ? "(bắt buộc)"
                    : "(tùy chọn)"}
                </span>
                <input
                  type="password"
                  autoComplete="off"
                  value={form.embeddingApiKey ?? ""}
                  onChange={(event) =>
                    set("embeddingApiKey", event.target.value || null)
                  }
                  placeholder="Không lưu xuống settings.json"
                />
              </label>
            </div>

            <Toggle
              checked={form.embeddingFallbackLocal}
              onChange={(checked) => set("embeddingFallbackLocal", checked)}
              label="Fallback local khi embedding lỗi"
              description="Rebuild scope bằng local hash; không trộn vector từ hai model."
            />

            {form.embeddingProvider === "ollama" && (
              <div className="local-setup">
                <code>ollama pull {form.embeddingModel || "nomic-embed-text"}</code>
              </div>
            )}

            <div className="llm-test-row">
              <Button
                variant="secondary"
                icon={<Wifi size={14} />}
                loading={testingEmbedding}
                disabled={validation.length > 0}
                onClick={testEmbeddingConnection}
              >
                Test embedding
              </Button>
              {embeddingResult && (
                <span className={embeddingResult.local ? "local-result" : ""}>
                  OK · {embeddingResult.model} · {embeddingResult.dimensions}D ·{" "}
                  {embeddingResult.latencyMs}ms
                </span>
              )}
            </div>
          </div>
        )}

        {!!validation.length && (
          <div className="form-errors" role="alert">
            {validation.map((message) => (
              <div key={message}>{message}</div>
            ))}
          </div>
        )}
      </div>
    </Modal>
  );
}
