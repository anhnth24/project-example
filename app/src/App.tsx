import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AlertCircle, Upload } from "lucide-react";
import { useStore } from "./state/store";
import { findByRel } from "./lib/tree";
import { Sidebar } from "./components/Sidebar";
import { DocView } from "./components/DocView";
import { SettingsModal } from "./components/Settings";
import { IconRail } from "./components/IconRail";
import { DocumentTabs } from "./components/DocumentTabs";
import { HomeView } from "./components/HomeView";
import { LibraryView } from "./components/LibraryView";
import { CommandPalette } from "./components/CommandPalette";
import { ConvertQueue } from "./components/ConvertQueue";
import { Button, IconButton, Modal } from "./components/ui";

const IntelligenceView = lazy(() =>
  import("./components/IntelligenceView").then((module) => ({
    default: module.IntelligenceView,
  })),
);

export default function App() {
  const init = useStore((state) => state.init);
  const tree = useStore((state) => state.tree);
  const view = useStore((state) => state.view);
  const setView = useStore((state) => state.setView);
  const activeTab = useStore((state) => state.activeTab);
  const sessions = useStore((state) => state.sessions);
  const jobs = useStore((state) => state.jobs);
  const supportedExts = useStore((state) => state.supportedExts);
  const importSources = useStore((state) => state.importSources);
  const saveSession = useStore((state) => state.saveSession);
  const closeTab = useStore((state) => state.closeTab);
  const error = useStore((state) => state.error);
  const setError = useStore((state) => state.setError);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [drawerOpen, setDrawerOpen] = useState(true);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [queueOpen, setQueueOpen] = useState(false);
  const [closeRequest, setCloseRequest] = useState<string | null>(null);

  const activeNode = useMemo(
    () => (activeTab ? findByRel(tree, activeTab) : null),
    [activeTab, tree],
  );
  const activeJobs = jobs.filter(
    (job) => job.status === "queued" || job.status === "running",
  ).length;
  const openSettings = useCallback(() => setSettingsOpen(true), []);

  useEffect(() => {
    void init();
  }, [init]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    (async () => {
      try {
        const stop = await getCurrentWebview().onDragDropEvent(async (event) => {
          const t = event.payload.type;
          if (t === "over" || t === "enter") setDragging(true);
          else if (t === "leave") setDragging(false);
          else if (t === "drop") {
            setDragging(false);
            await useStore.getState().importSources(event.payload.paths ?? []);
          }
        });
        if (disposed) stop();
        else unlisten = stop;
      } catch {
        // Browser-only preview: native drag paths are unavailable.
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const command = event.ctrlKey || event.metaKey;
      if (command && event.key.toLocaleLowerCase("vi") === "k") {
        event.preventDefault();
        setPaletteOpen(true);
      } else if (command && event.key.toLocaleLowerCase("vi") === "s") {
        event.preventDefault();
        if (view === "intelligence") {
          window.dispatchEvent(new Event("markhand:intelligence-save"));
        } else {
          void saveSession();
        }
      } else if (command && event.key.toLocaleLowerCase("vi") === "w" && activeTab) {
        event.preventDefault();
        requestCloseTab(activeTab);
      } else if (event.key === "Escape") {
        setPaletteOpen(false);
        setQueueOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  useEffect(() => {
    if (!error) return;
    const t = setTimeout(() => setError(null), 6000);
    return () => clearTimeout(t);
  }, [error, setError]);

  async function uploadFiles() {
    const picked = await openDialog({
      multiple: true,
      title: "Chọn file để thêm vào Markhand",
      filters: [{ name: "Định dạng hỗ trợ", extensions: supportedExts }],
    });
    if (!picked) return;
    await importSources(Array.isArray(picked) ? picked : [picked]);
    setDrawerOpen(true);
  }

  function requestCloseTab(relPath: string) {
    if (sessions[relPath]?.dirty) {
      setCloseRequest(relPath);
    } else {
      closeTab(relPath);
    }
  }

  const requestedNode = closeRequest ? findByRel(tree, closeRequest) : null;

  return (
    <div className="app-shell">
      <div className="starfield" aria-hidden="true" />
      <IconRail
        view={view}
        drawerOpen={drawerOpen}
        activeJobs={activeJobs}
        onHome={() => setView("home")}
        onLibrary={() => setView("library")}
        onIntelligence={() => setView("intelligence")}
        onToggleDrawer={() => setDrawerOpen((open) => !open)}
        onSearch={() => setPaletteOpen(true)}
        onQueue={() => setQueueOpen((open) => !open)}
        onSettings={openSettings}
      />

      {drawerOpen && <Sidebar onOpenSettings={openSettings} />}

      <div className="workspace">
        {view !== "intelligence" && <DocumentTabs onRequestClose={requestCloseTab} />}
        <main className="main-content">
          {view === "intelligence" ? (
            <Suspense fallback={<div className="docview doc-loading">Đang tải Intelligence…</div>}>
              <IntelligenceView />
            </Suspense>
          ) : view === "library" ? (
            <LibraryView onUpload={uploadFiles} />
          ) : view === "document" && activeNode && !activeNode.isDir ? (
            <DocView node={activeNode} />
          ) : (
            <HomeView
              onUpload={uploadFiles}
              onDocuments={() => setDrawerOpen(true)}
            />
          )}
        </main>
      </div>

      {dragging && (
        <div className="drop-overlay">
          <div className="drop-overlay-box">
            <Upload size={34} />
            <div className="drop-overlay-title">Thả để thêm vào thư mục đích</div>
            <div className="drop-overlay-sub">PDF · Word · Excel · PPT · CSV · HTML · ảnh · audio</div>
          </div>
        </div>
      )}

      {queueOpen && <ConvertQueue onClose={() => setQueueOpen(false)} />}
      {paletteOpen && (
        <CommandPalette
          onClose={() => setPaletteOpen(false)}
          onOpenSettings={openSettings}
        />
      )}
      {settingsOpen && <SettingsModal onClose={() => setSettingsOpen(false)} />}

      {closeRequest && (
        <Modal
          title={`Đóng “${requestedNode?.name ?? "tài liệu"}”?`}
          description="Tab này có thay đổi Markdown chưa được lưu."
          onClose={() => setCloseRequest(null)}
          width={430}
          footer={
            <>
              <Button variant="ghost" onClick={() => setCloseRequest(null)}>
                Tiếp tục sửa
              </Button>
              <Button
                variant="danger"
                onClick={() => {
                  closeTab(closeRequest);
                  setCloseRequest(null);
                }}
              >
                Bỏ thay đổi và đóng
              </Button>
              <Button
                variant="primary"
                onClick={async () => {
                  await saveSession(closeRequest);
                  if (!useStore.getState().sessions[closeRequest]?.dirty) {
                    closeTab(closeRequest);
                    setCloseRequest(null);
                  }
                }}
              >
                Lưu và đóng
              </Button>
            </>
          }
        >
          <div className="unsaved-summary">
            Bản nháp vẫn còn trong tab hiện tại. Chọn “Lưu và đóng” để ghi vào file
            Markdown liên kết.
          </div>
        </Modal>
      )}

      {error && (
        <div className="error-toast" role="alert">
          <AlertCircle size={16} />
          <span>{error}</span>
          <IconButton label="Đóng thông báo" onClick={() => setError(null)}>
            ×
          </IconButton>
        </div>
      )}
    </div>
  );
}
