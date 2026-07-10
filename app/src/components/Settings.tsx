import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderOpen, RotateCcw } from "lucide-react";
import { useStore } from "../state/store";
import type { Settings } from "../lib/types";
import { Button, Modal, Toggle } from "./ui";

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
  const [saving, setSaving] = useState(false);

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

  return (
    <Modal
      title="Cài đặt convert"
      description="Cấu hình được lưu trên máy và áp dụng cho các job chưa bắt đầu cùng những job mới."
      onClose={onClose}
      width={520}
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
