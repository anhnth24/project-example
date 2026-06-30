import { useEffect, useState } from "react";
import { FileText, FolderPlus, Upload, Columns2 } from "lucide-react";
import { EmptyState } from "@astryxdesign/core/EmptyState";
import { Card } from "@astryxdesign/core/Card";
import { Banner } from "@astryxdesign/core/Banner";
import { Icon } from "@astryxdesign/core/Icon";
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

  useEffect(() => {
    if (!error) return;
    const t = setTimeout(() => setError(null), 6000);
    return () => clearTimeout(t);
  }, [error, setError]);

  return (
    <div className="app">
      <Sidebar onOpenSettings={() => setSettingsOpen(true)} />

      <main className="main">
        {selected && !selected.isDir ? (
          <DocView key={selected.relPath} node={selected} />
        ) : (
          <HomeState />
        )}
      </main>

      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}

      {error && (
        <div className="toast-wrap">
          <Banner
            status="error"
            title={error}
            isDismissable
            onDismiss={() => setError(null)}
          />
        </div>
      )}
    </div>
  );
}

function HomeState() {
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
    <div className="home">
      <EmptyState
        icon={<Icon icon={FileText} size="lg" />}
        title="Markhand"
        description="Biến mọi tài liệu nguồn thành Markdown sạch để bàn giao cho Dev — dành cho BA & PM."
      />
      <div className="home-steps">
        {steps.map((s, i) => (
          <Card key={i} padding={4}>
            <div className="step-icon">{s.icon}</div>
            <div className="step-num">Bước {i + 1}</div>
            <div className="step-title">{s.title}</div>
            <div className="step-desc">{s.desc}</div>
          </Card>
        ))}
      </div>
    </div>
  );
}
