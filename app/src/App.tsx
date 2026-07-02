import { useEffect, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FileText, FolderPlus, Upload, Columns2 } from "lucide-react";
import { EmptyState } from "@astryxdesign/core/EmptyState";
import { Card } from "@astryxdesign/core/Card";
import { Banner } from "@astryxdesign/core/Banner";
import { Icon } from "@astryxdesign/core/Icon";
import { useStore } from "./state/store";
import { api } from "./lib/ipc";
import { Sidebar } from "./components/Sidebar";
import { DocView } from "./components/DocView";
import { SettingsModal } from "./components/Settings";

export default function App() {
  const init = useStore((s) => s.init);
  const error = useStore((s) => s.error);
  const setError = useStore((s) => s.setError);
  const selected = useStore((s) => s.selected);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    init();
  }, [init]);

  // Kéo-thả file vào BẤT KỲ đâu trong cửa sổ → import vào thư mục đích
  // (pattern Smallpdf/Notion: toàn cửa sổ là drop target, overlay khi kéo qua).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      try {
        unlisten = await getCurrentWebview().onDragDropEvent(async (event) => {
          const t = event.payload.type;
          if (t === "over" || t === "enter") setDragging(true);
          else if (t === "leave") setDragging(false);
          else if (t === "drop") {
            setDragging(false);
            const { activeFolder, refreshTree, setError } = useStore.getState();
            const errors: string[] = [];
            for (const p of event.payload.paths ?? []) {
              try {
                await api.importFile(activeFolder, p);
              } catch (e) {
                errors.push(String(e));
              }
            }
            await refreshTree();
            if (errors.length) setError(errors.join(" • "));
          }
        });
      } catch {
        // Ngoài môi trường Tauri (dev browser) — bỏ qua.
      }
    })();
    return () => unlisten?.();
  }, []);

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

      {dragging && (
        <div className="drop-overlay">
          <div className="drop-overlay-box">
            <Upload size={34} />
            <div className="drop-overlay-title">Thả để thêm vào thư mục đích</div>
            <div className="drop-overlay-sub">PDF · Word · Excel · PPT · CSV · HTML · ảnh · audio</div>
          </div>
        </div>
      )}

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
