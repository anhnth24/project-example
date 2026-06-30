import { useState } from "react";
import { ChevronRight, Folder, FolderOpen, Pencil, Trash2 } from "lucide-react";
import { IconButton } from "@astryxdesign/core/IconButton";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { fileIcon } from "../lib/icons";
import type { FsNode } from "../lib/types";

export function Tree({ node, depth }: { node: FsNode; depth: number }) {
  const [open, setOpen] = useState(depth < 1);
  const selected = useStore((s) => s.selected);
  const selectNode = useStore((s) => s.selectNode);
  const refreshTree = useStore((s) => s.refreshTree);
  const setError = useStore((s) => s.setError);

  const isSelected = selected?.relPath === node.relPath;
  const unconverted = !node.isDir && node.supported && !node.mdRelPath;

  async function onDelete(e: React.MouseEvent) {
    e.stopPropagation();
    const what = node.isDir ? "thư mục (và toàn bộ bên trong)" : "file";
    if (!confirm(`Xóa ${what} "${node.name}"?`)) return;
    try {
      await api.deleteNode(node.relPath);
      await refreshTree();
    } catch (err) {
      setError(String(err));
    }
  }

  async function onRename(e: React.MouseEvent) {
    e.stopPropagation();
    const next = prompt("Tên mới:", node.name);
    if (!next || next === node.name) return;
    try {
      await api.renameNode(node.relPath, next.trim());
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
        style={{ paddingLeft: 8 + depth * 14 }}
        onClick={onClick}
        title={node.relPath}
      >
        <span className={`twisty ${node.isDir && open ? "open" : ""}`}>
          {node.isDir && <ChevronRight size={14} />}
        </span>
        <span className="row-icon">
          {node.isDir ? (
            open ? <FolderOpen size={16} color="#e0a83e" /> : <Folder size={16} color="#e0a83e" />
          ) : (
            fileIcon(node)
          )}
        </span>
        <span className="row-label">{node.name}</span>
        {unconverted && <span className="dot" title="Chưa convert" />}
        <span className="row-actions">
          <IconButton label="Đổi tên" tooltip="Đổi tên" variant="ghost" size="sm" icon={<Pencil size={13} />} onClick={onRename} />
          <IconButton label="Xóa" tooltip="Xóa" variant="ghost" size="sm" icon={<Trash2 size={13} />} onClick={onDelete} />
        </span>
      </div>
      {node.isDir && open && node.children.length > 0 && (
        <div className="children">
          {node.children.map((c) => (
            <Tree key={c.relPath} node={c} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}
