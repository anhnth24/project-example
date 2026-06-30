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
    <Dialog isOpen onOpenChange={(open: boolean) => !open && onClose()} width={520}>
      <Layout
        header={<DialogHeader title="Cài đặt convert" onOpenChange={(open: boolean) => !open && onClose()} />}
        content={
          <div className="settings-form">
            <TextInput label="Ngôn ngữ OCR (ảnh / PDF scan)" value={form.ocrLangs} onChange={(v: string) => set("ocrLangs", v)} placeholder="vie+eng" />
            <CheckboxInput label="OCR trang PDF dạng scan (ít/không có lớp text)" value={form.pdfOcr} onChange={(v: boolean) => set("pdfOcr", v)} />
            <CheckboxInput label="OCR thêm ảnh nhúng trong trang PDF có text (chậm hơn)" value={form.pdfOcrImages} onChange={(v: boolean) => set("pdfOcrImages", v)} />
            <div className="settings-grid">
              <TextInput label="Ngôn ngữ audio" value={form.audioLang} onChange={(v: string) => set("audioLang", v)} />
              <NumberInput label="Thread audio" value={form.audioThreads} onChange={(v: number) => set("audioThreads", v || 1)} min={1} max={32} />
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
              <Button label="Hủy" variant="ghost" onClick={onClose} />
              <Button label="Lưu" variant="primary" onClick={onSave} />
            </div>
          </LayoutFooter>
        }
      />
    </Dialog>
  );
}
