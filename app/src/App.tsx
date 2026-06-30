import { useEffect, useState } from "react";
import { useStore } from "./state/store";
import { Sidebar } from "./components/Sidebar";
import { DocView } from "./components/DocView";
import { SettingsModal } from "./components/Settings";

export default function App() {
  const init = useStore((s) => s.init);
  const error = useStore((s) => s.error);
  const setError = useStore((s) => s.setError);
  const selected = useStore((s) => s.selected);
  const workspaces = useStore((s) => s.workspaces);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    init();
  }, [init]);

  return (
    <div className="app">
      <Sidebar onOpenSettings={() => setSettingsOpen(true)} />

      <main className="main">
        {selected && !selected.isDir ? (
          <DocView key={selected.relPath} node={selected} />
        ) : (
          <EmptyState hasWorkspace={workspaces.length > 0} />
        )}
      </main>

      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}

      {error && (
        <div className="toast" role="alert">
          <span>{error}</span>
          <button onClick={() => setError(null)}>✕</button>
        </div>
      )}
    </div>
  );
}

function EmptyState({ hasWorkspace }: { hasWorkspace: boolean }) {
  return (
    <div className="empty">
      <h1>FileConv Docs</h1>
      <p>Soạn tài liệu cho Dev từ file gốc → Markdown, lưu hoàn toàn ở máy bạn.</p>
      <ol>
        {!hasWorkspace && <li>Bấm <b>＋ Workspace</b> ở thanh bên để chọn một thư mục làm việc.</li>}
        <li>Tạo <b>thư mục</b> trong workspace.</li>
        <li>Bấm <b>＋ File</b> để đưa file gốc vào — app tự tạo file <code>.md</code> liên kết 1-1.</li>
        <li>Chọn file để <b>xem song song</b> (gốc ↔ markdown) hoặc <b>sửa</b> markdown.</li>
      </ol>
    </div>
  );
}
