import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderPlus, Upload, FilePlus2, Settings as SettingsIcon, FolderCog, Folder, Sun, Moon } from "lucide-react";
import { useTheme } from "../lib/theme";
import { Button } from "@astryxdesign/core/Button";
import { IconButton } from "@astryxdesign/core/IconButton";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { Tree } from "./Tree";

export function Sidebar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const { dataRoot, tree, activeFolder, supportedExts, refreshTree, changeDataRoot, setError } =
    useStore();
  const [theme, toggleTheme] = useTheme();

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
        <span className="brand-mark">A&rarr;M</span>
        <span className="brand-name">Markhand</span>
      </div>

      <div className="data-card" title={dataRoot}>
        <Folder size={14} className="data-icon" />
        <div className="data-text">
          <div className="data-label">Thư mục dữ liệu</div>
          <div className="data-path">{dataRoot || "…"}</div>
        </div>
        <IconButton label="Đổi thư mục DATA" tooltip="Đổi thư mục DATA" variant="ghost" size="sm" icon={<FolderCog size={16} />} onClick={changeDir} />
      </div>

      <div className="toolbar-row">
        <div className="toolbar-grow">
          <Button label="Tải file" variant="primary" size="sm" icon={<Upload size={15} />} onClick={uploadFiles} />
        </div>
        <IconButton label="Thư mục mới" tooltip="Thư mục mới" variant="secondary" size="sm" icon={<FolderPlus size={15} />} onClick={newFolder} />
        <IconButton label="Markdown mới" tooltip="Markdown mới" variant="secondary" size="sm" icon={<FilePlus2 size={15} />} onClick={newMarkdown} />
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

      <div className="sidebar-foot">
        <Button label="Cài đặt convert" variant="ghost" size="sm" icon={<SettingsIcon size={15} />} onClick={onOpenSettings} />
        <IconButton
          label={theme === "dark" ? "Giao diện sáng" : "Giao diện tối"}
          tooltip={theme === "dark" ? "Chuyển giao diện sáng" : "Chuyển giao diện tối"}
          variant="ghost"
          size="sm"
          icon={theme === "dark" ? <Sun size={15} /> : <Moon size={15} />}
          onClick={toggleTheme}
        />
      </div>
    </aside>
  );
}
