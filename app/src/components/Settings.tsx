import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { X } from "lucide-react";
import { useStore } from "../state/store";
import type { Settings } from "../lib/types";

const DEFAULTS: Settings = {
  ocrLangs: "vie+eng",
  pdfOcr: true,
  pdfOcrImages: false,
  audioLang: "vi",
  audioThreads: 4,
  whisperModel: null,
};

export function SettingsModal({ onClose }: { onClose: () => void }) {
  const current = useStore((s) => s.settings) ?? DEFAULTS;
  const saveSettings = useStore((s) => s.saveSettings);
  const setError = useStore((s) => s.setError);
  const [form, setForm] = useState<Settings>(current);

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
    await saveSettings(form);
    onClose();
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>Cài đặt convert</h2>
          <button className="ghost-icon" onClick={onClose}>
            <X size={18} />
          </button>
        </div>

        <label className="field">
          <span>Ngôn ngữ OCR (ảnh / PDF scan)</span>
          <input value={form.ocrLangs} onChange={(e) => set("ocrLangs", e.target.value)} placeholder="vie+eng" />
        </label>

        <label className="field check">
          <input type="checkbox" checked={form.pdfOcr} onChange={(e) => set("pdfOcr", e.target.checked)} />
          <span>OCR trang PDF dạng scan (ít/không có lớp text)</span>
        </label>

        <label className="field check">
          <input
            type="checkbox"
            checked={form.pdfOcrImages}
            onChange={(e) => set("pdfOcrImages", e.target.checked)}
          />
          <span>OCR thêm ảnh nhúng trong trang PDF có text (chậm hơn)</span>
        </label>

        <hr />

        <div className="field-grid">
          <label className="field">
            <span>Ngôn ngữ audio</span>
            <input value={form.audioLang} onChange={(e) => set("audioLang", e.target.value)} />
          </label>
          <label className="field">
            <span>Thread audio</span>
            <input
              type="number"
              min={1}
              max={32}
              value={form.audioThreads}
              onChange={(e) => set("audioThreads", Number(e.target.value) || 1)}
            />
          </label>
        </div>

        <label className="field">
          <span>Model whisper (.bin) — để trống nếu không dùng audio</span>
          <div className="row-inline">
            <input
              value={form.whisperModel ?? ""}
              onChange={(e) => set("whisperModel", e.target.value || null)}
              placeholder="đường dẫn tới ggml-*.bin"
            />
            <button className="btn-ghost" onClick={pickWhisper}>
              Chọn…
            </button>
          </div>
        </label>

        <div className="modal-actions">
          <button className="btn-ghost" onClick={onClose}>
            Hủy
          </button>
          <button className="btn-primary" onClick={onSave}>
            Lưu
          </button>
        </div>
      </div>
    </div>
  );
}
