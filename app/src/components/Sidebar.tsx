import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  FilePlus2,
  Folder,
  FolderCog,
  FolderPlus,
  Search,
  Settings,
  Upload,
} from "lucide-react";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { isWithinRel, parentRel } from "../lib/tree";
import type { FsNode } from "../lib/types";
import { Tree } from "./Tree";
import { Button, IconButton, Modal } from "./ui";

type DialogState =
  | { kind: "create-folder" | "create-markdown" }
  | { kind: "rename" | "delete"; node: FsNode }
  | null;

export function Sidebar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const {
    dataRoot,
    tree,
    activeFolder,
    supportedExts,
    sessions,
    jobs,
    activeImports,
    workspaceChanging,
    refreshTree,
    changeDataRoot,
    importSources,
    openNode,
    closeTabsWithin,
    setActiveFolder,
    setError,
  } = useStore();
  const [query, setQuery] = useState("");
  const [dialog, setDialog] = useState<DialogState>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);

  async function changeDir() {
    if (Object.values(sessions).some((session) => session.dirty)) {
      setError("Hãy lưu hoặc đóng các tab đang chỉnh sửa trước khi đổi thư mục DATA.");
      return;
    }
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: "Chọn thư mục DATA",
    });
    if (picked && !Array.isArray(picked)) await changeDataRoot(picked);
  }

  function openCreate(kind: "create-folder" | "create-markdown") {
    setName("");
    setDialog({ kind });
  }

  function openRename(node: FsNode) {
    if (mutationBlocked(node)) return;
    setName(node.name);
    setDialog({ kind: "rename", node });
  }

  function openDelete(node: FsNode) {
    if (mutationBlocked(node)) return;
    setDialog({ kind: "delete", node });
  }

  function mutationBlocked(node: FsNode): boolean {
    const dirty = Object.values(sessions).some(
      (session) => session.dirty && isWithinRel(session.relPath, node.relPath),
    );
    const working =
      activeImports > 0 ||
      workspaceChanging ||
      jobs.some(
        (job) =>
          (job.status === "queued" || job.status === "running") &&
          isWithinRel(job.relPath, node.relPath),
      );
    if (dirty) {
      setError("Hãy lưu hoặc đóng các tab thuộc mục này trước khi đổi tên hoặc xóa.");
      return true;
    }
    if (working) {
      setError("Không thể đổi tên hoặc xóa khi file liên quan đang import/convert.");
      return true;
    }
    return false;
  }

  async function submitDialog() {
    if (!dialog || dialog.kind === "delete") return;
    const value = name.trim();
    if (!value) return;
    if (dialog.kind === "rename" && mutationBlocked(dialog.node)) return;
    setBusy(true);
    try {
      if (dialog.kind === "create-folder") {
        await api.createFolder(activeFolder, value);
      } else if (dialog.kind === "create-markdown") {
        const node = await api.createMarkdown(activeFolder, value);
        await refreshTree();
        openNode(node);
        setDialog(null);
        return;
      } else {
        await api.renameNode(dialog.node.relPath, value);
        closeTabsWithin(dialog.node.relPath);
        if (dialog.node.isDir && isWithinRel(activeFolder, dialog.node.relPath)) {
          setActiveFolder(parentRel(dialog.node.relPath));
        }
      }
      await refreshTree();
      setDialog(null);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function confirmDelete() {
    if (!dialog || dialog.kind !== "delete") return;
    if (mutationBlocked(dialog.node)) return;
    setBusy(true);
    try {
      await api.deleteNode(dialog.node.relPath);
      closeTabsWithin(dialog.node.relPath);
      if (dialog.node.isDir && isWithinRel(activeFolder, dialog.node.relPath)) {
        setActiveFolder(parentRel(dialog.node.relPath));
      }
      await refreshTree();
      setDialog(null);
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  }

  async function uploadFiles() {
    const picked = await openDialog({
      multiple: true,
      title: "Chọn file để thêm vào Markhand",
      filters: [{ name: "Định dạng hỗ trợ", extensions: supportedExts }],
    });
    if (!picked) return;
    const files = Array.isArray(picked) ? picked : [picked];
    await importSources(files);
  }

  const rootName = dataRoot.split(/[/\\]/).filter(Boolean).pop() || "DATA";
  const folderLabel = activeFolder === "" ? rootName : activeFolder.split("/").pop();

  return (
    <aside className="document-drawer">
      <div className="drawer-heading">
        <div>
          <span className="eyebrow">Không gian làm việc</span>
          <strong>Tài liệu</strong>
        </div>
        <span className="drawer-count">{tree?.children.length ?? 0}</span>
      </div>

      <label className="drawer-search">
        <Search size={14} />
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Lọc file, thư mục…"
          aria-label="Lọc file và thư mục"
        />
        <kbd>⌘K</kbd>
      </label>

      <div className="drawer-actions">
        <Button variant="primary" size="sm" icon={<Upload size={14} />} onClick={uploadFiles}>
          Tải file
        </Button>
        <IconButton label="Tạo thư mục" onClick={() => openCreate("create-folder")}>
          <FolderPlus size={14} />
        </IconButton>
        <IconButton label="Tạo Markdown" onClick={() => openCreate("create-markdown")}>
          <FilePlus2 size={14} />
        </IconButton>
      </div>

      <div className="drawer-section-label">
        <span>DATA</span>
        <span title={`File mới sẽ vào ${folderLabel}`}>Đích: {folderLabel}</span>
      </div>

      <div className="tree-scroll">
        {tree && tree.children.length ? (
          tree.children.map((child) => (
            <Tree
              key={child.relPath}
              node={child}
              depth={0}
              query={query}
              onRename={openRename}
              onDelete={openDelete}
            />
          ))
        ) : (
          <div className="drawer-empty">Trống — tải file hoặc tạo thư mục để bắt đầu.</div>
        )}
      </div>

      <div className="data-root-card" title={dataRoot}>
        <Folder size={15} />
        <span>
          <small>Thư mục dữ liệu</small>
          <b>{dataRoot || "…"}</b>
        </span>
        <IconButton label="Đổi thư mục DATA" onClick={changeDir}>
          <FolderCog size={14} />
        </IconButton>
      </div>
      <button type="button" className="drawer-settings" onClick={onOpenSettings}>
        <Settings size={14} />
        Cài đặt convert
      </button>

      {dialog && dialog.kind !== "delete" && (
        <Modal
          title={
            dialog.kind === "rename"
              ? "Đổi tên"
              : dialog.kind === "create-folder"
                ? "Tạo thư mục"
                : "Tạo file Markdown"
          }
          onClose={() => setDialog(null)}
          width={420}
          footer={
            <>
              <Button variant="ghost" onClick={() => setDialog(null)}>
                Hủy
              </Button>
              <Button variant="primary" loading={busy} onClick={submitDialog}>
                {dialog.kind === "rename" ? "Đổi tên" : "Tạo"}
              </Button>
            </>
          }
        >
          <label className="field">
            <span>Tên</span>
            <input
              autoFocus
              value={name}
              onChange={(event) => setName(event.target.value)}
              onKeyDown={(event) => event.key === "Enter" && void submitDialog()}
              placeholder={dialog.kind === "create-markdown" ? "ghi-chu-ban-giao.md" : "Tên mới"}
            />
          </label>
        </Modal>
      )}

      {dialog?.kind === "delete" && (
        <Modal
          title={`Xóa “${dialog.node.name}”?`}
          description={
            dialog.node.isDir
              ? "Toàn bộ nội dung trong thư mục cũng sẽ bị xóa. Thao tác này không thể hoàn tác."
              : "File Markdown được liên kết cũng sẽ bị xóa. Thao tác này không thể hoàn tác."
          }
          onClose={() => setDialog(null)}
          width={440}
          footer={
            <>
              <Button variant="ghost" onClick={() => setDialog(null)}>
                Hủy
              </Button>
              <Button variant="danger" loading={busy} onClick={confirmDelete}>
                Xóa
              </Button>
            </>
          }
        >
          <div className="delete-warning">
            Hãy chắc chắn tài liệu này không còn cần cho việc bàn giao.
          </div>
        </Modal>
      )}
    </aside>
  );
}
