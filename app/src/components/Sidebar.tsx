import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  FilePlus2,
  Filter,
  Folder,
  FolderCog,
  FolderInput,
  FolderPlus,
  PanelsTopLeft,
  Plus,
  Search,
  Settings,
  Upload,
} from "lucide-react";
import { useStore, type SortOption } from "../state/store";
import { api } from "../lib/ipc";
import { findByRel, isWithinRel, parentRel } from "../lib/tree";
import type { FsNode } from "../lib/types";
import { Tree, sortChildren, hasVisibleDescendant } from "./Tree";
import { Button, IconButton, Modal, SelectControl } from "./ui";

type DialogState =
  | { kind: "create-folder" }
  | { kind: "create-markdown" }
  | { kind: "create-project" }
  | { kind: "rename"; node: FsNode }
  | { kind: "delete"; node: FsNode }
  | null;

function isSortOption(val: string): val is SortOption {
  return [
    "name_asc",
    "name_desc",
    "type_asc",
    "type_desc",
    "converted_first",
    "unconverted_first",
  ].includes(val);
}

export function Sidebar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const dataRoot = useStore((state) => state.dataRoot);
  const tree = useStore((state) => state.tree);
  const projects = useStore((state) => state.projects);
  const activeProjectId = useStore((state) => state.activeProjectId);
  const activeFolder = useStore((state) => state.activeFolder);
  const supportedExts = useStore((state) => state.supportedExts);
  const sessions = useStore((state) => state.sessions);
  const jobs = useStore((state) => state.jobs);
  const activeImports = useStore((state) => state.activeImports);
  const workspaceChanging = useStore((state) => state.workspaceChanging);
  const refreshTree = useStore((state) => state.refreshTree);
  const refreshProjects = useStore((state) => state.refreshProjects);
  const setActiveProject = useStore((state) => state.setActiveProject);
  const changeDataRoot = useStore((state) => state.changeDataRoot);
  const importSources = useStore((state) => state.importSources);
  const openNode = useStore((state) => state.openNode);
  const closeTabsWithin = useStore((state) => state.closeTabsWithin);
  const setActiveFolder = useStore((state) => state.setActiveFolder);
  const setError = useStore((state) => state.setError);
  const sortBy = useStore((state) => state.sortBy);
  const setSortBy = useStore((state) => state.setSortBy);
  const filterUnconvertedOnly = useStore((state) => state.filterUnconvertedOnly);
  const setFilterUnconvertedOnly = useStore((state) => state.setFilterUnconvertedOnly);
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

  const activeProject =
    projects.find((project) => project.id === activeProjectId) ?? null;
  const projectNode =
    activeProject?.rootRel && tree
      ? findByRel(tree, activeProject.rootRel)
      : tree;

  const baseChildren = (projectNode?.children ?? []).filter(
    (child) =>
      activeProject?.rootRel !== "" ||
      !projects.some(
        (project) =>
          project.rootRel !== "" &&
          project.rootRel.toLowerCase() === child.relPath.toLowerCase(),
      )
  );

  const projectChildren = sortChildren(
    baseChildren.filter((child) =>
      hasVisibleDescendant(child, query, filterUnconvertedOnly)
    ),
    sortBy,
  );

  function openCreate(
    kind: "create-folder" | "create-markdown" | "create-project",
  ) {
    setName("");
    setDialog({ kind });
  }

  async function importLocalFolder() {
    if (!activeProject) {
      setError("Hãy tạo hoặc chọn dự án trước khi import folder.");
      return;
    }
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: `Import folder local vào ${activeProject.name}`,
    });
    if (!picked || Array.isArray(picked)) return;
    setBusy(true);
    try {
      const result = await api.importLocalFolder(
        activeProject.id,
        picked,
        activeFolder,
      );
      await refreshTree();
      await refreshProjects();
      if (result.convertRels.length) {
        useStore.getState().enqueueConversions(result.convertRels);
      }
      if (result.skipped) {
        setError(
          `Đã import ${result.imported} file, bỏ qua ${result.skipped} file trùng.`,
        );
      }
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
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
      } else if (dialog.kind === "create-project") {
        const project = await api.createProject(value);
        await refreshProjects();
        await refreshTree();
        useStore.getState().setActiveProject(project.id);
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
        <span className="drawer-count">{projectChildren.length}</span>
      </div>

      <div className="project-switcher">
        <PanelsTopLeft size={14} />
        <div className="project-switcher-control">
          <span>Dự án</span>
          <SelectControl
            value={activeProjectId ?? ""}
            onChange={setActiveProject}
            ariaLabel="Chọn dự án"
            compact
            disabled={!projects.length}
            options={
              projects.length
                ? projects.map((project) => ({
                    value: project.id,
                    label: `${project.name}${project.implicit ? " · legacy" : ""}`,
                  }))
                : [{ value: "", label: "Chưa có dự án", disabled: true }]
            }
          />
        </div>
        <IconButton label="Tạo dự án" onClick={() => openCreate("create-project")}>
          <Plus size={14} />
        </IconButton>
        <IconButton
          label="Import folder local vào dự án"
          disabled={!activeProject || busy}
          onClick={importLocalFolder}
        >
          <FolderInput size={14} />
        </IconButton>
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
        <Button
          variant="primary"
          size="sm"
          icon={<Upload size={14} />}
          disabled={!activeProject}
          onClick={uploadFiles}
        >
          Tải file
        </Button>
        <IconButton
          label="Tạo thư mục"
          disabled={!activeProject}
          onClick={() => openCreate("create-folder")}
        >
          <FolderPlus size={14} />
        </IconButton>
        <IconButton
          label="Tạo Markdown"
          disabled={!activeProject}
          onClick={() => openCreate("create-markdown")}
        >
          <FilePlus2 size={14} />
        </IconButton>
      </div>

      <div className="drawer-section-label" style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <span>DATA</span>
        <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
          <span title={`File mới sẽ vào ${folderLabel}`}>Đích: {folderLabel}</span>
          <div style={{ display: "inline-flex", alignItems: "center", gap: "4px" }}>
            <IconButton
              label={filterUnconvertedOnly ? "Hiển thị tất cả file" : "Chỉ hiện file chưa convert"}
              active={filterUnconvertedOnly}
              onClick={() => setFilterUnconvertedOnly(!filterUnconvertedOnly)}
            >
              <Filter size={14} />
            </IconButton>
            <div title="Phân loại file theo" style={{ display: "inline-flex" }}>
              <SelectControl
                value={sortBy}
                onChange={(val) => {
                  if (isSortOption(val)) {
                    setSortBy(val);
                  }
                }}
                ariaLabel="Sắp xếp"
                placeholder="Phân loại file theo"
                compact
                options={[
                  { value: "name_asc", label: "Tên (A-Z)" },
                  { value: "name_desc", label: "Tên (Z-A)" },
                  { value: "type_asc", label: "Loại file (A-Z)" },
                  { value: "type_desc", label: "Loại file (Z-A)" },
                  { value: "converted_first", label: "Đã convert trước" },
                  { value: "unconverted_first", label: "Chưa convert trước" },
                ]}
              />
            </div>
          </div>
        </div>
      </div>

      <div className="tree-scroll">
        {!baseChildren.length ? (
          <div className="drawer-empty">
            {activeProject
              ? "Dự án trống — import folder, tải file hoặc tạo thư mục."
              : "Tạo dự án đầu tiên để bắt đầu."}
          </div>
        ) : !projectChildren.length ? (
          <div className="drawer-empty">
            Không có file khớp bộ lọc/tìm kiếm.
          </div>
        ) : (
          projectChildren.map((child) => (
            <Tree
              key={child.relPath}
              node={child}
              depth={0}
              query={query}
              onRename={openRename}
              onDelete={openDelete}
            />
          ))
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
                : dialog.kind === "create-markdown"
                  ? "Tạo file Markdown"
                  : "Tạo dự án"
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
              placeholder={
                dialog.kind === "create-markdown"
                  ? "ghi-chu-ban-giao.md"
                  : dialog.kind === "create-project"
                    ? "VD: Core Banking 2026"
                    : "Tên mới"
              }
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
