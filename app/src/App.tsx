import { useEffect, useState } from "react";
import { FileText, FolderPlus, Upload, Columns2, X } from "lucide-react";
import { useStore } from "./state/store";
import { Sidebar } from "./components/Sidebar";
import { DocView } from "./components/DocView";
import { SettingsModal } from "./components/Settings";

export default function App() {
  const init = useStore((s) => s.init);
  const error = useStore((s) => s.error);
  const setError = useStore((s) => s.setError);
  const selected = useStore((s) => s.selected);
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
          <EmptyState />
        )}
      </main>

      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}

      {error && (
        <div className="toast" role="alert">
          <span>{error}</span>
          <button className="toast-close" onClick={() => setError(null)}>
            <X size={15} />
          </button>
        </div>
      )}
    </div>
  );
}

function EmptyState() {
  const steps = [
    {
      icon: <FolderPlus size={18} />,
      title: "Tạo thư mục",
      desc: "Tổ chức tài liệu theo từng nhóm nghiệp vụ trong DATA.",
    },
    {
      icon: <Upload size={18} />,
      title: "Tải file gốc lên",
      desc: "PDF, Word, Excel, PPT, ảnh, audio… App tự convert sang Markdown (link 1-1).",
    },
    {
      icon: <Columns2 size={18} />,
      title: "Xem song song & sửa",
      desc: "Đối chiếu file gốc ↔ Markdown, chỉnh sửa rồi lưu — tất cả ở máy bạn.",
    },
  ];
  return (
    <div className="empty">
      <div className="empty-badge">
        <FileText size={28} />
      </div>
      <h1>FileConv Docs</h1>
      <p className="empty-sub">
        Biến tài liệu nguồn thành Markdown sạch để bàn giao cho Dev — dành cho BA & PM.
      </p>
      <div className="empty-steps">
        {steps.map((s, i) => (
          <div className="step-card" key={i}>
            <div className="step-icon">{s.icon}</div>
            <div className="step-num">Bước {i + 1}</div>
            <div className="step-title">{s.title}</div>
            <div className="step-desc">{s.desc}</div>
          </div>
        ))}
      </div>
    </div>
  );
}
