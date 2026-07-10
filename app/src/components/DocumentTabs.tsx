import { X } from "lucide-react";
import { fileIcon } from "../lib/icons";
import { findByRel } from "../lib/tree";
import { useStore } from "../state/store";

export function DocumentTabs({
  onRequestClose,
}: {
  onRequestClose: (relPath: string) => void;
}) {
  const tree = useStore((state) => state.tree);
  const openTabs = useStore((state) => state.openTabs);
  const activeTab = useStore((state) => state.activeTab);
  const sessions = useStore((state) => state.sessions);
  const view = useStore((state) => state.view);
  const openNode = useStore((state) => state.openNode);

  if (!openTabs.length) return null;

  return (
    <nav className="document-tabs" aria-label="Tài liệu đang mở">
      {openTabs.map((relPath) => {
        const node = findByRel(tree, relPath);
        if (!node) return null;
        const active = view === "document" && activeTab === relPath;
        return (
          <div
            key={relPath}
            className={`document-tab ${active ? "active" : ""}`}
          >
            <button
              type="button"
              className="tab-main"
              aria-current={active ? "page" : undefined}
              onClick={() => openNode(node)}
            >
              {fileIcon(node, { size: 13 })}
              <span>{node.name}</span>
              {sessions[relPath]?.dirty && (
                <span className="dirty-dot" title="Chưa lưu" aria-label="Chưa lưu" />
              )}
            </button>
            <button
              type="button"
              className="tab-close"
              onClick={() => onRequestClose(relPath)}
              aria-label={`Đóng ${node.name}`}
              title="Đóng tab"
            >
              <X size={12} />
            </button>
          </div>
        );
      })}
    </nav>
  );
}
