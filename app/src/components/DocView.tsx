import { useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import type { FsNode } from "../lib/types";
import { SourcePreview } from "./SourcePreview";
import { MarkdownEditor } from "./MarkdownEditor";

type Mode = "split" | "md" | "source";

export function DocView({ node }: { node: FsNode }) {
  const currentWsId = useStore((s) => s.currentWsId)!;
  const refreshTree = useStore((s) => s.refreshTree);
  const setError = useStore((s) => s.setError);

  const isStandaloneMd = node.standaloneMd;
  const canSource = !isStandaloneMd;
  const mdRel = node.mdRelPath;
  const canMd = !!mdRel;
  const canConvert = canSource && node.supported;

  const [mode, setMode] = useState<Mode>(
    canSource && canMd ? "split" : canMd ? "md" : "source"
  );
  const [md, setMd] = useState("");
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [converting, setConverting] = useState(false);

  // Nạp nội dung markdown khi đổi file hoặc sau khi convert (mdRel xuất hiện).
  useEffect(() => {
    let alive = true;
    if (mdRel) {
      api
        .readTextFile(currentWsId, mdRel)
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
  }, [currentWsId, mdRel, setError]);

  async function save() {
    if (!mdRel || !dirty) return;
    setSaving(true);
    try {
      await api.writeTextFile(currentWsId, mdRel, md);
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
      await api.reconvert(currentWsId, node.relPath);
      await refreshTree(); // node.mdRelPath sẽ được cập nhật -> effect nạp lại md.
    } catch (e) {
      setError(String(e));
    } finally {
      setConverting(false);
    }
  }

  async function openExternal() {
    try {
      const abs = await api.resolvePath(currentWsId, node.relPath);
      await openPath(abs);
    } catch (e) {
      setError(String(e));
    }
  }

  // Ctrl/Cmd+S để lưu.
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
        <div className="doc-title" title={node.relPath}>
          {node.name}
          {dirty && <span className="dirty">●</span>}
        </div>

        <div className="modes">
          {canSource && canMd && (
            <button className={mode === "split" ? "on" : ""} onClick={() => setMode("split")}>
              Song song
            </button>
          )}
          {canMd && (
            <button className={mode === "md" ? "on" : ""} onClick={() => setMode("md")}>
              Markdown
            </button>
          )}
          {canSource && (
            <button className={mode === "source" ? "on" : ""} onClick={() => setMode("source")}>
              File gốc
            </button>
          )}
        </div>

        <div className="doc-actions">
          {canMd && (
            <button onClick={save} disabled={!dirty || saving}>
              {saving ? "Đang lưu…" : "Lưu"}
            </button>
          )}
          {canConvert && (
            <button onClick={convert} disabled={converting}>
              {converting ? "Đang convert…" : mdRel ? "Convert lại" : "Convert"}
            </button>
          )}
          {canSource && (
            <button onClick={openExternal} title="Mở file gốc bằng ứng dụng mặc định">
              Mở ngoài
            </button>
          )}
        </div>
      </header>

      <div className={`doc-body ${mode}`}>
        {(mode === "split" || mode === "source") && canSource && (
          <div className="pane source-pane">
            <SourcePreview workspaceId={currentWsId} node={node} onError={setError} />
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
                    <p>File này chưa có bản Markdown.</p>
                    <button onClick={convert} disabled={converting}>
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
