import {
  FolderOpen,
  House,
  LayoutGrid,
  RefreshCw,
  Search,
  Sparkles,
  Settings,
} from "lucide-react";
import type { AppView } from "../lib/types";
import { IconButton } from "./ui";

export function IconRail({
  view,
  drawerOpen,
  activeJobs,
  onHome,
  onLibrary,
  onIntelligence,
  onToggleDrawer,
  onSearch,
  onQueue,
  onSettings,
}: {
  view: AppView;
  drawerOpen: boolean;
  activeJobs: number;
  onHome: () => void;
  onLibrary: () => void;
  onIntelligence: () => void;
  onToggleDrawer: () => void;
  onSearch: () => void;
  onQueue: () => void;
  onSettings: () => void;
}) {
  return (
    <nav className="icon-rail" aria-label="Điều hướng chính">
      <button className="brand-orb" type="button" onClick={onHome} aria-label="Trang chủ Markhand">
        A→M
      </button>
      <IconButton label="Trang chủ" active={view === "home"} onClick={onHome}>
        <House size={17} />
      </IconButton>
      <IconButton label="Tài liệu" active={drawerOpen} onClick={onToggleDrawer}>
        <FolderOpen size={17} />
      </IconButton>
      <IconButton label="Thư viện" active={view === "library"} onClick={onLibrary}>
        <LayoutGrid size={17} />
      </IconButton>
      <IconButton
        label="Bàn giao và Intelligence"
        active={view === "intelligence"}
        onClick={onIntelligence}
      >
        <Sparkles size={17} />
      </IconButton>
      <IconButton label="Tìm kiếm (Ctrl+K)" onClick={onSearch}>
        <Search size={17} />
      </IconButton>
      <IconButton
        label="Hàng đợi convert"
        badge={activeJobs}
        className={activeJobs ? "rail-progress" : ""}
        onClick={onQueue}
      >
        <RefreshCw size={17} />
      </IconButton>
      <span className="rail-spacer" />
      <IconButton label="Cài đặt convert" onClick={onSettings}>
        <Settings size={17} />
      </IconButton>
    </nav>
  );
}
