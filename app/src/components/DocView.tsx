import { useState, type ReactNode } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  Check,
  Columns2,
  Copy,
  ExternalLink,
  FileCode2,
  FileInput,
  GitCompareArrows,
  LoaderCircle,
  RefreshCw,
  Save,
} from "lucide-react";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { fileIcon } from "../lib/icons";
import type { DocumentMode, FsNode } from "../lib/types";
import { SourcePreview } from "./SourcePreview";
import { MarkdownEditor } from "./MarkdownEditor";
import { CompareView } from "./CompareView";
import { Button } from "./ui";

export function DocView({ node }: { node: FsNode }) {
  const session = useStore((state) => state.sessions[node.relPath]);
  const updateDraft = useStore((state) => state.updateDraft);
  const setDocumentMode = useStore((state) => state.setDocumentMode);
  const setMarkdownTab = useStore((state) => state.setMarkdownTab);
  const saveSession = useStore((state) => state.saveSession);
  const enqueueConversions = useStore((state) => state.enqueueConversions);
  const jobs = useStore((state) => state.jobs);
  const setError = useStore((state) => state.setError);

  const isStandaloneMd = node.standaloneMd;
  const canSource = !isStandaloneMd;
  const mdRel = node.mdRelPath;
  const canMd = !!mdRel;
  const canConvert = canSource && node.supported;
  const [copied, setCopied] = useState(false);
  const converting = jobs.some(
    (job) =>
      job.relPath === node.relPath &&
      (job.status === "queued" || job.status === "running"),
  );

  async function save() {
    if (!mdRel || !session?.dirty) return;
    await saveSession(node.relPath);
  }

  async function copyMarkdown() {
    try {
      await navigator.clipboard.writeText(session?.draft ?? "");
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (error) {
      setError(String(error));
    }
  }

  function convert() {
    enqueueConversions([node.relPath]);
  }

  async function openExternal() {
    try {
      await openPath(await api.resolvePath(node.relPath));
    } catch (error) {
      setError(String(error));
    }
  }

  if (!session?.loaded) {
    return (
      <div className="docview doc-loading">
        <LoaderCircle className="spin" size={22} />
        Đang mở tài liệu…
      </div>
    );
  }

  const availableModes: { value: DocumentMode; label: string; icon: ReactNode }[] = [];
  if (canSource && canMd) {
    availableModes.push(
      { value: "compare", label: "Đối chiếu", icon: <GitCompareArrows size={13} /> },
      { value: "split", label: "Song song", icon: <Columns2 size={13} /> },
    );
  }
  if (canMd) {
    availableModes.push({
      value: "markdown",
      label: "Markdown",
      icon: <FileCode2 size={13} />,
    });
  }
  if (canSource) {
    availableModes.push({
      value: "source",
      label: "File gốc",
      icon: <FileInput size={13} />,
    });
  }
  const effectiveMode = availableModes.some((mode) => mode.value === session.mode)
    ? session.mode
    : (availableModes[0]?.value ?? "markdown");

  return (
    <div className="docview">
      <header className="doc-toolbar">
        <div className="doc-title">
          <span className="doc-title-icon">{fileIcon(node, { size: 18 })}</span>
          <span className="doc-title-copy">
            <span className="doc-title-name">{node.name}</span>
            <small>
              {node.kind.toUpperCase()} · {canMd ? `${session.draft.length.toLocaleString("vi-VN")} ký tự` : "chưa convert"}
            </small>
          </span>
          {session.dirty && <span className="dirty-dot" title="Chưa lưu" />}
        </div>

        <div className="doc-modes">
          <div className="segmented-control" aria-label="Chế độ xem tài liệu">
            {availableModes.map((mode) => (
              <button
                type="button"
                aria-pressed={effectiveMode === mode.value}
                className={effectiveMode === mode.value ? "active" : ""}
                key={mode.value}
                onClick={() => setDocumentMode(node.relPath, mode.value)}
              >
                {mode.icon}
                {mode.label}
              </button>
            ))}
          </div>
        </div>

        <div className="doc-actions">
          {converting ? (
            <span className="doc-status">Đang convert</span>
          ) : session.dirty ? (
            <span className="doc-status">Chưa lưu</span>
          ) : session.savedAt ? (
            <span className="doc-status">Đã lưu {session.savedAt}</span>
          ) : null}
          {canMd && (
            <Button variant="ghost" size="sm" icon={copied ? <Check size={14} /> : <Copy size={14} />} onClick={copyMarkdown}>
              {copied ? "Đã copy" : "Copy MD"}
            </Button>
          )}
          {canConvert && (
            <Button
              variant="secondary"
              size="sm"
              icon={<RefreshCw size={14} />}
              loading={converting}
              disabled={session.dirty}
              onClick={convert}
            >
              {mdRel ? "Convert lại" : "Convert"}
            </Button>
          )}
          {canMd && (
            <Button
              variant="primary"
              size="sm"
              icon={<Save size={14} />}
              loading={session.saving}
              disabled={!session.dirty || converting}
              onClick={save}
            >
              Lưu Markdown
            </Button>
          )}
          {canSource && (
            <Button
              className="open-external-button"
              variant="ghost"
              size="sm"
              icon={<ExternalLink size={14} />}
              onClick={openExternal}
            >
              Mở ngoài
            </Button>
          )}
        </div>
      </header>

      {!canMd && canSource ? (
        <div className="doc-body split raw-document">
          <div className="pane source-pane">
            <SourcePreview node={node} onError={setError} />
          </div>
          <div className="pane md-pane">
            <div className="placeholder">
              <RefreshCw size={28} className={converting ? "spin" : "placeholder-icon"} />
              <p>{converting ? `Đang convert ${node.name}…` : "File này chưa có bản Markdown."}</p>
              {canConvert && !converting && (
                <Button variant="primary" onClick={convert}>
                  Convert ngay
                </Button>
              )}
              {converting && <span className="indeterminate-track"><span /></span>}
            </div>
          </div>
        </div>
      ) : effectiveMode === "compare" ? (
        <CompareView
          node={node}
          session={session}
          readOnly={converting}
          onChange={(markdown) => updateDraft(node.relPath, markdown)}
        />
      ) : (
        <div className={`doc-body ${effectiveMode}`}>
          {(effectiveMode === "split" || effectiveMode === "source") && canSource && (
            <div className="pane source-pane">
              <SourcePreview node={node} onError={setError} />
            </div>
          )}
          {(effectiveMode === "split" || effectiveMode === "markdown") && canMd && (
            <div className="pane md-pane">
              <MarkdownEditor
                value={session.draft}
                onChange={(markdown) => updateDraft(node.relPath, markdown)}
                readOnly={converting}
                tab={session.markdownTab}
                onTabChange={(tab) => setMarkdownTab(node.relPath, tab)}
              />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
