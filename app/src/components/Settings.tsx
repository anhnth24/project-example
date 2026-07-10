import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Dialog, DialogHeader } from "@astryxdesign/core/Dialog";
import { Layout, LayoutFooter } from "@astryxdesign/core/Layout";
import { Button } from "@astryxdesign/core/Button";
import { TextInput } from "@astryxdesign/core/TextInput";
import { NumberInput } from "@astryxdesign/core/NumberInput";
import { CheckboxInput } from "@astryxdesign/core/CheckboxInput";
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

// Form nháp: audioThreads có thể rỗng (null) trong lúc gõ, chỉ Settings đã validate mới lưu.
type SettingsForm = Omit<Settings, "audioThreads"> & { audioThreads: number | null };

type FormErrors = Partial<Record<"ocrLangs" | "audioLang" | "audioThreads", string>>;

// Mã ngôn ngữ tesseract: vie, eng, chi_sim… nối bằng "+"
const OCR_LANGS_RE = /^[a-z_]+(\+[a-z_]+)*$/i;

function validate(form: SettingsForm): FormErrors {
  const errors: FormErrors = {};
  if (!form.ocrLangs.trim() || !OCR_LANGS_RE.test(form.ocrLangs.trim())) {
    errors.ocrLangs = "Ngôn ngữ OCR không hợp lệ (ví dụ: vie+eng)";
  }
  if (!form.audioLang.trim()) {
    errors.audioLang = "Ngôn ngữ audio không được để trống";
  }
  if (form.audioThreads == null || Number.isNaN(form.audioThreads) || form.audioThreads < 1 || form.audioThreads > 32) {
    errors.audioThreads = "Thread audio phải từ 1 đến 32";
  }
  return errors;
}

export function SettingsModal({ onClose }: { onClose: () => void }) {
  const current = useStore((s) => s.settings) ?? DEFAULTS;
  const saveSettings = useStore((s) => s.saveSettings);
  const setError = useStore((s) => s.setError);
  const [form, setForm] = useState<SettingsForm>(current);

  const errors = validate(form);
  const hasErrors = Object.keys(errors).length > 0;

  function set<K extends keyof SettingsForm>(key: K, val: SettingsForm[K]) {
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
    if (hasErrors) return;
    await saveSettings(form as Settings);
    onClose();
  }

  return (
    <Dialog isOpen onOpenChange={(open: boolean) => !open && onClose()} width={520}>
      <Layout
        header={<DialogHeader title="Cài đặt convert" onOpenChange={(open: boolean) => !open && onClose()} />}
        content={
          <div className="settings-form">
            <TextInput
              label="Ngôn ngữ OCR (ảnh / PDF scan)"
              value={form.ocrLangs}
              onChange={(v: string) => set("ocrLangs", v)}
              placeholder="vie+eng"
              status={errors.ocrLangs ? { type: "error", message: errors.ocrLangs } : undefined}
            />
            <CheckboxInput label="OCR trang PDF dạng scan (ít/không có lớp text)" value={form.pdfOcr} onChange={(v: boolean) => set("pdfOcr", v)} />
            <CheckboxInput label="OCR thêm ảnh nhúng trong trang PDF có text (chậm hơn)" value={form.pdfOcrImages} onChange={(v: boolean) => set("pdfOcrImages", v)} />
            <div className="settings-grid">
              <TextInput
                label="Ngôn ngữ audio"
                value={form.audioLang}
                onChange={(v: string) => set("audioLang", v)}
                status={errors.audioLang ? { type: "error", message: errors.audioLang } : undefined}
              />
              <NumberInput
                label="Thread audio"
                value={form.audioThreads}
                // hasClear để onChange nhận được null khi xoá — không có nó NumberInput
                // nuốt giá trị rỗng/sai và tự revert, validate không bao giờ thấy.
                hasClear
                onChange={(v: number | null) => set("audioThreads", v)}
                min={1}
                max={32}
                status={errors.audioThreads ? { type: "error", message: errors.audioThreads } : undefined}
              />
            </div>
            <div className="settings-whisper">
              <TextInput label="Model whisper (.bin) — để trống nếu không dùng audio" value={form.whisperModel ?? ""} onChange={(v: string) => set("whisperModel", v || null)} placeholder="đường dẫn tới ggml-*.bin" />
              <Button label="Chọn…" variant="secondary" onClick={pickWhisper} />
            </div>
          </div>
        }
        footer={
          <LayoutFooter hasDivider>
            <div className="settings-actions">
              <Button label="Khôi phục mặc định" variant="ghost" onClick={() => setForm(DEFAULTS)} />
              <Button label="Hủy" variant="ghost" onClick={onClose} />
              <Button label="Lưu" variant="primary" onClick={onSave} isDisabled={hasErrors} />
            </div>
          </LayoutFooter>
        }
      />
    </Dialog>
  );
}
