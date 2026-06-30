import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { Tree } from "./Tree";

export function Sidebar({ onOpenSettings }: { onOpenSettings: () => void }) {
  const {
    workspaces,
    currentWsId,
    tree,
    activeFolder,
    supportedExts,
    selectWorkspace,
    refreshTree,
    setError,
  } = useStore();

  async function addWorkspace() {
    try {
      const picked = await openDialog({ directory: true, multiple: false, title: "Chọn thư mục workspace" });
      if (!picked || Array.isArray(picked)) return;
      const ws = await api.addWorkspace(picked);
      // Nạp lại danh sách rồi chọn workspace vừa thêm.
      const list = await api.listWorkspaces();
      useStore.setState({ workspaces: list });
      await selectWorkspace(ws.id);
    } catch (e) {
      setError(String(e));
    }
  }

  async function removeWorkspace() {
    if (!currentWsId) return;
    const ws = workspaces.find((w) => w.id === currentWsId);
    if (!ws) return;
    if (!confirm(`Gỡ workspace "${ws.name}" khỏi danh sách? (KHÔNG xóa file trên đĩa)`)) return;
    try {
      await api.removeWorkspace(currentWsId);
      const list = await api.listWorkspaces();
      useStore.setState({ workspaces: list, tree: null, selected: null });
      if (list.length > 0) await selectWorkspace(list[0].id);
      else useStore.setState({ currentWsId: null });
    } catch (e) {
      setError(String(e));
    }
  }

  async function newFolder() {
    if (!currentWsId) return;
    const name = prompt("Tên thư mục mới:");
    if (!name) return;
    try {
      await api.createFolder(currentWsId, activeFolder, name.trim());
      await refreshTree();
    } catch (e) {
      setError(String(e));
    }
  }

  async function newMarkdown() {
    if (!currentWsId) return;
    const name = prompt("Tên file markdown mới (không cần .md):");
    if (!name) return;
    try {
      const node = await api.createMarkdown(currentWsId, activeFolder, name.trim());
      await refreshTree();
      useStore.getState().selectNode(node);
    } catch (e) {
      setError(String(e));
    }
  }

  async function importFiles() {
    if (!currentWsId) return;
    try {
      const picked = await openDialog({
        multiple: true,
        title: "Chọn file để đưa vào folder",
        filters: [{ name: "Định dạng hỗ trợ", extensions: supportedExts }],
      });
      if (!picked) return;
      const files = Array.isArray(picked) ? picked : [picked];
      const errors: string[] = [];
      for (const f of files) {
        try {
          await api.importFile(currentWsId, activeFolder, f);
        } catch (e) {
          errors.push(String(e));
        }
      }
      await refreshTree();
      if (errors.length) setError(errors.join(" • "));
    } catch (e) {
      setError(String(e));
    }
  }

  const folderLabel =
    activeFolder === "" ? "(gốc workspace)" : activeFolder.split("/").pop();

  return (
    <aside className="sidebar">
      <div className="ws-bar">
        <select
          className="ws-select"
          value={currentWsId ?? ""}
          onChange={(e) => selectWorkspace(e.target.value)}
        >
          {workspaces.length === 0 && <option value="">— chưa có workspace —</option>}
          {workspaces.map((w) => (
            <option key={w.id} value={w.id}>
              {w.name}
            </option>
          ))}
        </select>
        <button className="icon-btn" title="Thêm workspace" onClick={addWorkspace}>
          ＋
        </button>
        {currentWsId && (
          <button className="icon-btn" title="Gỡ workspace" onClick={removeWorkspace}>
            🗑
          </button>
        )}
      </div>

      {currentWsId && (
        <>
          <div className="actions">
            <button onClick={importFiles} title="Đưa file gốc vào folder hiện tại">
              ＋ File
            </button>
            <button onClick={newFolder}>＋ Thư mục</button>
            <button onClick={newMarkdown} title="Tạo file markdown trống">
              ＋ MD
            </button>
          </div>
          <div className="active-folder" title="Đích cho thao tác tạo/đưa file">
            📁 {folderLabel}
          </div>
        </>
      )}

      <div className="tree-scroll">
        {tree ? (
          tree.children.length ? (
            tree.children.map((c) => <Tree key={c.relPath} node={c} depth={0} />)
          ) : (
            <div className="hint">Trống. Bấm ＋ File hoặc ＋ Thư mục.</div>
          )
        ) : (
          <div className="hint">Chọn hoặc thêm một workspace.</div>
        )}
      </div>

      <div className="sidebar-footer">
        <button className="link-btn" onClick={onOpenSettings}>
          ⚙ Cài đặt convert
        </button>
      </div>
    </aside>
  );
}
