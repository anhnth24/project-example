import { useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { Columns2, FileText, Image as ImageIcon, Save, RefreshCw, ExternalLink } from "lucide-react";
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

  useEffect(() => {
    let alive = true;
    if (mdRel) {
      api
        .readTextFile(mdRel)
        .then((t) => {
          if (alive) {
            setMd(t);
            setDirty(false);
          }
        })
        .catch((e) => setError(String(e)));
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
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
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

  const seg = (m: Mode, icon: React.ReactNode, label: string) => (
    <button className={`seg ${mode === m ? "on" : ""}`} onClick={() => setMode(m)}>
      {icon} {label}
    </button>
  );

  return (
    <div className="docview">
      <header className="doc-toolbar">
        <div className="doc-title">
          <span className="doc-title-icon">{fileIcon(node, { size: 18 })}</span>
          <span className="doc-title-name">{node.name}</span>
          {dirty && <span className="dirty-dot" title="Chưa lưu" />}
        </div>

        <div className="segmented">
          {canSource && canMd && seg("split", <Columns2 size={15} />, "Song song")}
          {canMd && seg("md", <FileText size={15} />, "Markdown")}
          {canSource && seg("source", <ImageIcon size={15} />, "File gốc")}
        </div>

        <div className="doc-actions">
          {canMd && (
            <button className="btn-primary sm" onClick={save} disabled={!dirty || saving}>
              <Save size={15} /> {saving ? "Đang lưu…" : "Lưu"}
            </button>
          )}
          {canConvert && (
            <button className="btn-ghost sm" onClick={convert} disabled={converting}>
              <RefreshCw size={15} className={converting ? "spin" : ""} />
              {mdRel ? "Convert lại" : "Convert"}
            </button>
          )}
          {canSource && (
            <button className="btn-ghost sm" onClick={openExternal} title="Mở bằng app mặc định">
              <ExternalLink size={15} /> Mở ngoài
            </button>
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
                    <button className="btn-primary" onClick={convert} disabled={converting}>
                      {converting ? "Đang convert…" : "Convert ngay"}
                    </button>
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
