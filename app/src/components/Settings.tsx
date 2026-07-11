import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Cloud, FolderOpen, RotateCcw, Server, Wifi } from "lucide-react";
import { api } from "../lib/ipc";
import { applyLlmPreset, validateLlmSettings } from "../lib/llmSettings";
import { useStore } from "../state/store";
import type {
  LlmConnectionResult,
  LlmProviderPreset,
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
  pdfOcr: true,
  pdfOcrImages: false,
  audioLang: "vi",
  audioThreads: 4,
  whisperModel: null,
  llmEnabled: false,
  llmProvider: "ollama",
  llmBaseUrl: "http://127.0.0.1:11434",
  llmModel: "qwen2.5:7b",
  llmApiKey: null,
};

function providerOptionLabel(preset: LlmProviderPreset): string {
  const cleanLabel = preset.label.replace(/\s*\((?:Local|Self-host)\)$/i, "");
  return `${preset.local ? "Local" : "Cloud"} · ${cleanLabel}`;
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

  useEffect(() => {
    api
      .getLlmProviderPresets()
      .then(setPresets)
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

  function applyPreset(id: string) {
    const preset = presets.find((item) => item.id === id);
    if (!preset) {
      set("llmProvider", id);
      return;
    }
    setForm((currentForm) => applyLlmPreset(currentForm, preset));
    setLlmResult(null);
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
  validation.push(...validateLlmSettings(form, selectedPreset));

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
        <label className="field">
          <span>Ngôn ngữ OCR (ảnh / PDF scan)</span>
          <input
            value={form.ocrLangs}
            onChange={(event) => set("ocrLangs", event.target.value)}
            placeholder="vie+eng"
          />
          <small>Có thể ghép các mã Tesseract bằng dấu “+”.</small>
        </label>

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

        <div className="settings-grid">
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
              <Notice tone={selectedPreset.local ? "info" : "warning"}>
                <span className="provider-notice">
                  {selectedPreset.local ? <Server size={14} /> : <Cloud size={14} />}
                  <span>
                    <b>
                      {selectedPreset.local
                        ? "100% local — khuyến nghị"
                        : "Dữ liệu top citation sẽ rời máy"}
                    </b>
                    <small>{selectedPreset.description}</small>
                  </span>
                </span>
              </Notice>
            )}

            <div className="settings-grid llm-grid">
              <label className="field">
                <span>Base URL</span>
                <input
                  value={form.llmBaseUrl}
                  onChange={(event) => set("llmBaseUrl", event.target.value)}
                  placeholder="http://127.0.0.1:11434"
                />
              </label>
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
            </div>

            <label className="field">
              <span>API key {selectedPreset?.requiresApiKey ? "(bắt buộc)" : "(tùy chọn)"}</span>
              <input
                type="password"
                autoComplete="off"
                value={form.llmApiKey ?? ""}
                onChange={(event) => set("llmApiKey", event.target.value || null)}
                placeholder={
                  selectedPreset?.local
                    ? "Local provider thường không cần key"
                    : "Chỉ giữ trong memory; dùng FILECONV_LLM_API_KEY để persist"
                }
              />
              <small>
                Markhand không ghi API key vào settings.json. Sau khi restart, dùng biến môi
                trường hoặc nhập lại.
              </small>
            </label>

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
