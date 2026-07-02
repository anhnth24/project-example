import { useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { Save, RefreshCw, ExternalLink, Copy, Check } from "lucide-react";
import { Button } from "@astryxdesign/core/Button";
import { TabList, Tab } from "@astryxdesign/core/TabList";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { fileIcon } from "../lib/icons";
import type { FsNode } from "../lib/types";
import { SourcePreview } from "./SourcePreview";
import { MarkdownEditor } from "./MarkdownEditor";

type Mode = "split" | "md" | "source";

export function DocView({ node }: { node: FsNode }) {
  const refreshTree = useStore((s) => s.refreshTree);
  const setError = useStore((s) => s.setError);

  const isStandaloneMd = node.standaloneMd;
  const canSource = !isStandaloneMd;
  const mdRel = node.mdRelPath;
  const canMd = !!mdRel;
  const canConvert = canSource && node.supported;

  const [mode, setMode] = useState<Mode>(canSource && canMd ? "split" : canMd ? "md" : "source");
  const [md, setMd] = useState("");
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [converting, setConverting] = useState(false);
  const [savedAt, setSavedAt] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let alive = true;
    if (mdRel) {
      api.readTextFile(mdRel).then((t) => {
        if (alive) {
          setMd(t);
          setDirty(false);
        }
      }).catch((e) => setError(String(e)));
    } else {
      setMd("");
      setDirty(false);
    }
    return () => {
      alive = false;
    };
  }, [mdRel, setError]);

  async function save() {
    if (!mdRel || !dirty) return;
    setSaving(true);
    try {
      await api.writeTextFile(mdRel, md);
      setDirty(false);
      setSavedAt(new Date().toLocaleTimeString("vi-VN", { hour: "2-digit", minute: "2-digit" }));
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function copyMarkdown() {
    try {
      await navigator.clipboard.writeText(md);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      setError(String(e));
    }
  }

  async function convert() {
    setConverting(true);
    try {
      await api.reconvert(node.relPath);
      await refreshTree();
    } catch (e) {
      setError(String(e));
    } finally {
      setConverting(false);
    }
  }

  async function openExternal() {
    try {
      await openPath(await api.resolvePath(node.relPath));
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        save();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <div className="docview">
      <header className="doc-toolbar">
        <div className="doc-title">
          <span className="doc-title-icon">{fileIcon(node, { size: 18 })}</span>
          <span className="doc-title-name">{node.name}</span>
          {dirty && <span className="dirty-dot" title="Chưa lưu" />}
        </div>

        <div className="doc-modes">
          <TabList value={mode} onChange={(v: string) => setMode(v as Mode)}>
            {canSource && canMd ? <Tab value="split" label="Song song" /> : <></>}
            {canMd ? <Tab value="md" label="Markdown" /> : <></>}
            {canSource ? <Tab value="source" label="File gốc" /> : <></>}
          </TabList>
        </div>

        <div className="doc-actions">
          {canMd && (
            <span className="doc-meta">
              <span>{md.length.toLocaleString("vi-VN")} ký tự</span>
              {savedAt && !dirty && <span className="saved-at">Đã lưu {savedAt}</span>}
            </span>
          )}
          {canMd && (
            <Button
              label={copied ? "Đã copy" : "Copy MD"}
              variant="ghost"
              size="sm"
              icon={copied ? <Check size={15} /> : <Copy size={15} />}
              onClick={copyMarkdown}
            />
          )}
          {canMd && (
            <Button label={saving ? "Đang lưu…" : "Lưu"} variant="primary" size="sm" icon={<Save size={15} />} isDisabled={!dirty || saving} isLoading={saving} onClick={save} />
          )}
          {canConvert && (
            <Button label={mdRel ? "Convert lại" : "Convert"} variant="secondary" size="sm" icon={<RefreshCw size={15} />} isLoading={converting} onClick={convert} />
          )}
          {canSource && (
            <Button label="Mở ngoài" variant="ghost" size="sm" icon={<ExternalLink size={15} />} onClick={openExternal} />
          )}
        </div>
      </header>

      <div className={`doc-body ${mode}`}>
        {(mode === "split" || mode === "source") && canSource && (
          <div className="pane source-pane">
            <SourcePreview node={node} onError={setError} />
          </div>
        )}

        {(mode === "split" || mode === "md") && (
          <div className="pane md-pane">
            {canMd ? (
              <MarkdownEditor
                value={md}
                onChange={(v) => {
                  setMd(v);
                  setDirty(true);
                }}
              />
            ) : (
              <div className="placeholder">
                {canConvert ? (
                  <>
                    <RefreshCw size={28} className="placeholder-icon" />
                    <p>File này chưa có bản Markdown.</p>
                    <Button label={converting ? "Đang convert…" : "Convert ngay"} variant="primary" isLoading={converting} onClick={convert} />
                  </>
                ) : (
                  <p>Định dạng không hỗ trợ convert sang Markdown.</p>
                )}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
