import { useState } from "react";
import { ChevronRight, Folder, FolderOpen, Pencil, Trash2 } from "lucide-react";
import { useStore, type SortOption } from "../state/store";
import { fileIcon } from "../lib/icons";
import type { FsNode } from "../lib/types";
import { IconButton } from "./ui";
import { normalizeSearch } from "../lib/tree";

export function hasVisibleDescendant(
  node: FsNode,
  query: string,
  filterUnconvertedOnly: boolean
): boolean {
  const q = normalizeSearch(query);
  if (!node.isDir) {
    const matchesQuery = !q || normalizeSearch(node.name).includes(q);
    const matchesFilter =
      !filterUnconvertedOnly || !!(node.supported && !node.mdRelPath);
    return matchesQuery && matchesFilter;
  }
  // Folder: tên khớp query → hiện (kể cả trống / không có file match)
  if (q && normalizeSearch(node.name).includes(q)) return true;
  // Không query + không filter → hiện mọi folder (kể cả trống)
  if (!q && !filterUnconvertedOnly) return true;
  // Còn lại: hiện folder nếu có descendant còn visible
  return node.children.some((child) =>
    hasVisibleDescendant(child, query, filterUnconvertedOnly)
  );
}

export function hasUnconvertedDescendant(node: FsNode): boolean {
  return hasVisibleDescendant(node, "", true);
}

export function sortChildren(children: FsNode[], sortBy: SortOption): FsNode[] {
  return [...children].sort((a, b) => {
    if (a.isDir !== b.isDir) {
      return a.isDir ? -1 : 1;
    }

    const nameComp = a.name.localeCompare(b.name, "vi", { sensitivity: "base", numeric: true });

    if (sortBy === "name_asc") {
      return nameComp;
    }
    if (sortBy === "name_desc") {
      return -nameComp;
    }

    if (sortBy === "type_asc") {
      const kindA = a.kind || "";
      const kindB = b.kind || "";
      const comp = kindA.localeCompare(kindB, "vi");
      return comp !== 0 ? comp : nameComp;
    }
    if (sortBy === "type_desc") {
      const kindA = a.kind || "";
      const kindB = b.kind || "";
      const comp = kindB.localeCompare(kindA, "vi");
      return comp !== 0 ? comp : nameComp;
    }

    if (sortBy === "converted_first") {
      const unconvA = !a.isDir && a.supported && !a.mdRelPath;
      const unconvB = !b.isDir && b.supported && !b.mdRelPath;
      if (unconvA !== unconvB) {
        return unconvA ? 1 : -1;
      }
      return nameComp;
    }

    if (sortBy === "unconverted_first") {
      const unconvA = !a.isDir && a.supported && !a.mdRelPath;
      const unconvB = !b.isDir && b.supported && !b.mdRelPath;
      if (unconvA !== unconvB) {
        return unconvA ? -1 : 1;
      }
      return nameComp;
    }

    return nameComp;
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
  const filterUnconvertedOnly = useStore((state) => state.filterUnconvertedOnly);

  if (!hasVisibleDescendant(node, query, filterUnconvertedOnly)) return null;

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
          {sortChildren(
            node.children.filter((c) => hasVisibleDescendant(c, query, filterUnconvertedOnly)),
            sortBy
          ).map((c) => (
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
