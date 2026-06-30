import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  FileText,
  FolderPlus,
  Upload,
  FilePlus2,
  Settings as SettingsIcon,
  FolderCog,
  Folder,
} from "lucide-react";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { Tree } from "./Tree";

export function Sidebar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { dataRoot, tree, activeFolder, supportedExts, refreshTree, changeDataRoot, setError } =
    useStore();

  async function changeDir() {
    const picked = await openDialog({ directory: true, multiple: false, title: "Chọn thư mục DATA" });
    if (picked && !Array.isArray(picked)) await changeDataRoot(picked);
  }

  async function newFolder() {
    const name = prompt("Tên thư mục mới:");
    if (!name) return;
    try {
      await api.createFolder(activeFolder, name.trim());
      await refreshTree();
    } catch (e) {
      setError(String(e));
    }
  }

  async function newMarkdown() {
    const name = prompt("Tên file markdown mới (không cần .md):");
    if (!name) return;
    try {
      const node = await api.createMarkdown(activeFolder, name.trim());
      await refreshTree();
      useStore.getState().selectNode(node);
    } catch (e) {
      setError(String(e));
    }
  }

  async function uploadFiles() {
    const picked = await openDialog({
      multiple: true,
      title: "Chọn file để convert",
      filters: [{ name: "Định dạng hỗ trợ", extensions: supportedExts }],
    });
    if (!picked) return;
    const files = Array.isArray(picked) ? picked : [picked];
    const errors: string[] = [];
    for (const f of files) {
      try {
        await api.importFile(activeFolder, f);
      } catch (e) {
        errors.push(String(e));
      }
    }
    await refreshTree();
    if (errors.length) setError(errors.join(" • "));
  }

  const rootName = dataRoot.split(/[/\\]/).filter(Boolean).pop() || "DATA";
  const folderLabel = activeFolder === "" ? rootName : activeFolder.split("/").pop();

  return (
    <aside className="sidebar">
      <div className="brand">
        <span className="brand-mark">
          <FileText size={18} />
        </span>
        <span className="brand-name">FileConv Docs</span>
      </div>

      <div className="data-card" title={dataRoot}>
        <Folder size={14} className="data-icon" />
        <div className="data-text">
          <div className="data-label">Thư mục dữ liệu</div>
          <div className="data-path">{dataRoot || "…"}</div>
        </div>
        <button className="ghost-icon" title="Đổi thư mục DATA" onClick={changeDir}>
          <FolderCog size={16} />
        </button>
      </div>

      <div className="toolbar-row">
        <button className="btn-primary" onClick={uploadFiles}>
          <Upload size={15} /> Tải file
        </button>
        <button className="btn-ghost" title="Thư mục mới" onClick={newFolder}>
          <FolderPlus size={15} />
        </button>
        <button className="btn-ghost" title="Markdown mới" onClick={newMarkdown}>
          <FilePlus2 size={15} />
        </button>
      </div>

      <div className="dest-hint">
        Đích: <b>{folderLabel}</b>
      </div>

      <div className="tree-scroll">
        {tree && tree.children.length ? (
          tree.children.map((c) => <Tree key={c.relPath} node={c} depth={0} />)
        ) : (
          <div className="hint">Trống — tải file hoặc tạo thư mục để bắt đầu.</div>
        )}
      </div>

      <button className="sidebar-settings" onClick={onOpenSettings}>
        <SettingsIcon size={15} /> Cài đặt convert
      </button>
    </aside>
  );
}
