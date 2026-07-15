import { useState } from "react";
import { ChevronRight, Folder, FolderOpen, Pencil, Trash2 } from "lucide-react";
import { useStore, type SortOption } from "../state/store";
import { fileIcon } from "../lib/icons";
import { nodeMatches } from "../lib/tree";
import type { FsNode } from "../lib/types";
import { IconButton } from "./ui";

export function sortChildren(children: FsNode[], sortBy: SortOption): FsNode[] {
  return [...children].sort((a, b) => {
    if (a.isDir !== b.isDir) {
      return a.isDir ? -1 : 1;
    }
    if (sortBy === "type") {
      const kindA = a.kind || "";
      const kindB = b.kind || "";
      const comp = kindA.localeCompare(kindB, "vi");
      if (comp !== 0) return comp;
    } else if (sortBy === "converted") {
      const unconvA = !a.isDir && a.supported && !a.mdRelPath;
      const unconvB = !b.isDir && b.supported && !b.mdRelPath;
      if (unconvA !== unconvB) {
        return unconvA ? -1 : 1;
      }
    }
    return a.name.localeCompare(b.name, "vi", { sensitivity: "base", numeric: true });
  });
}

export function Tree({
  node,
  depth,
  query,
  onRename,
  onDelete,
}: {
  node: FsNode;
  depth: number;
  query: string;
  onRename: (node: FsNode) => void;
  onDelete: (node: FsNode) => void;
}) {
  const [open, setOpen] = useState(depth < 1);
  const activeTab = useStore((state) => state.activeTab);
  const activeFolder = useStore((state) => state.activeFolder);
  const view = useStore((state) => state.view);
  const openNode = useStore((state) => state.openNode);
  const sortBy = useStore((state) => state.sortBy);

  if (!nodeMatches(node, query)) return null;
  const expanded = query.trim() ? true : open;
  const isSelected = node.isDir
    ? activeFolder === node.relPath
    : view === "document" && activeTab === node.relPath;
  const unconverted = !node.isDir && node.supported && !node.mdRelPath;

  function onClick() {
    if (node.isDir) setOpen((o) => !o);
    openNode(node);
  }

  return (
    <div className="tree-node">
      <div className={`tree-row ${isSelected ? "selected" : ""}`}>
        <button
          type="button"
          className="tree-row-main"
          style={{ paddingLeft: 8 + depth * 14 }}
          onClick={onClick}
          title={node.relPath}
          aria-expanded={node.isDir ? expanded : undefined}
        >
          <span className={`twisty ${node.isDir && expanded ? "open" : ""}`}>
            {node.isDir && <ChevronRight size={13} />}
          </span>
          <span className="row-icon">
            {node.isDir ? (
              expanded ? (
                <FolderOpen className="folder-icon" size={15} />
              ) : (
                <Folder className="folder-icon" size={15} />
              )
            ) : (
              fileIcon(node, { size: 15 })
            )}
          </span>
          <span className="row-label">{node.name}</span>
          {unconverted && <span className="dot" title="Chưa convert" />}
        </button>
        <span className="row-actions">
          <IconButton label={`Đổi tên ${node.name}`} onClick={() => onRename(node)}>
            <Pencil size={12} />
          </IconButton>
          <IconButton label={`Xóa ${node.name}`} onClick={() => onDelete(node)}>
            <Trash2 size={12} />
          </IconButton>
        </span>
      </div>
      {node.isDir && expanded && node.children.length > 0 && (
        <div className="children">
          {sortChildren(node.children, sortBy).map((c) => (
            <Tree
              key={c.relPath}
              node={c}
              depth={depth + 1}
              query={query}
              onRename={onRename}
              onDelete={onDelete}
            />
          ))}
        </div>
      )}
    </div>
  );
}
