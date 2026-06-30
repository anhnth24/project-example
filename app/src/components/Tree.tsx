import { useState } from "react";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import type { FsNode } from "../lib/types";

function kindIcon(node: FsNode): string {
  if (node.isDir) return "📁";
  switch (node.kind) {
    case "pdf":
      return "📕";
    case "docx":
      return "📘";
    case "pptx":
      return "📙";
    case "xlsx":
      return "📗";
    case "csv":
      return "🔢";
    case "html":
      return "🌐";
    case "image":
      return "🖼";
    case "audio":
      return "🎵";
    case "markdown":
      return "📝";
    default:
      return "📄";
  }
}

export function Tree({ node, depth }: { node: FsNode; depth: number }) {
  const [open, setOpen] = useState(depth < 1);
  const selected = useStore((s) => s.selected);
  const currentWsId = useStore((s) => s.currentWsId);
  const selectNode = useStore((s) => s.selectNode);
  const refreshTree = useStore((s) => s.refreshTree);
  const setError = useStore((s) => s.setError);

  const isSelected = selected?.relPath === node.relPath;
  // File gốc đã đưa vào nhưng chưa có md (convert lỗi) -> nhắc người dùng.
  const unconverted = !node.isDir && node.supported && !node.mdRelPath;

  async function onDelete(e: React.MouseEvent) {
    e.stopPropagation();
    if (!currentWsId) return;
    const what = node.isDir ? "thư mục (và toàn bộ bên trong)" : "file";
    if (!confirm(`Xóa ${what} "${node.name}"?`)) return;
    try {
      await api.deleteNode(currentWsId, node.relPath);
      await refreshTree();
    } catch (err) {
      setError(String(err));
    }
  }

  async function onRename(e: React.MouseEvent) {
    e.stopPropagation();
    if (!currentWsId) return;
    const next = prompt("Tên mới:", node.name);
    if (!next || next === node.name) return;
    try {
      await api.renameNode(currentWsId, node.relPath, next.trim());
      await refreshTree();
    } catch (err) {
      setError(String(err));
    }
  }

  function onClick() {
    if (node.isDir) setOpen((o) => !o);
    selectNode(node);
  }

  return (
    <div className="tree-node">
      <div
        className={`row ${isSelected ? "selected" : ""}`}
        style={{ paddingLeft: 6 + depth * 14 }}
        onClick={onClick}
        title={node.relPath}
      >
        <span className="chevron">{node.isDir ? (open ? "▾" : "▸") : ""}</span>
        <span className="icon">{kindIcon(node)}</span>
        <span className="label">{node.name}</span>
        {unconverted && <span className="badge" title="Chưa convert">●</span>}
        <span className="row-actions">
          <button className="mini" title="Đổi tên" onClick={onRename}>
            ✎
          </button>
          <button className="mini" title="Xóa" onClick={onDelete}>
            ✕
          </button>
        </span>
      </div>
      {node.isDir && open && (
        <div className="children">
          {node.children.map((c) => (
            <Tree key={c.relPath} node={c} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}
