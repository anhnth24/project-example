import { create } from "zustand";
import { api } from "../lib/ipc";
import type { FsNode, Settings, Workspace } from "../lib/types";

/** Tìm node theo relPath trong cây (để giữ lựa chọn sau khi refresh). */
function findByRel(node: FsNode | null, rel: string): FsNode | null {
  if (!node) return null;
  if (node.relPath === rel) return node;
  for (const c of node.children) {
    const f = findByRel(c, rel);
    if (f) return f;
  }
  return null;
}

interface AppStore {
  workspaces: Workspace[];
  currentWsId: string | null;
  tree: FsNode | null;
  selected: FsNode | null;
  /** Thư mục đích cho thao tác tạo/import (relPath; "" = gốc). */
  activeFolder: string;
  settings: Settings | null;
  supportedExts: string[];
  error: string | null;
  busy: boolean;

  init: () => Promise<void>;
  setError: (e: string | null) => void;
  selectWorkspace: (id: string) => Promise<void>;
  refreshTree: () => Promise<void>;
  selectNode: (node: FsNode) => void;
  setActiveFolder: (rel: string) => void;
  saveSettings: (s: Settings) => Promise<void>;
}

export const useStore = create<AppStore>((set, get) => ({
  workspaces: [],
  currentWsId: null,
  tree: null,
  selected: null,
  activeFolder: "",
  settings: null,
  supportedExts: [],
  error: null,
  busy: false,

  setError: (e) => set({ error: e }),

  init: async () => {
    try {
      const [exts, settings, workspaces] = await Promise.all([
        api.supportedExtensions(),
        api.getSettings(),
        api.listWorkspaces(),
      ]);
      set({ supportedExts: exts, settings, workspaces });
      if (workspaces.length > 0) {
        await get().selectWorkspace(workspaces[0].id);
      }
    } catch (e) {
      set({ error: String(e) });
    }
  },

  selectWorkspace: async (id) => {
    set({ currentWsId: id, selected: null, activeFolder: "", tree: null });
    await get().refreshTree();
  },

  refreshTree: async () => {
    const id = get().currentWsId;
    if (!id) return;
    try {
      const tree = await api.readTree(id);
      // Giữ lựa chọn cũ nếu node còn tồn tại.
      const prev = get().selected;
      const stillThere = prev ? findByRel(tree, prev.relPath) : null;
      set({ tree, selected: stillThere });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  selectNode: (node) => {
    if (node.isDir) {
      set({ selected: node, activeFolder: node.relPath });
    } else {
      // Thư mục đích = thư mục cha của file.
      const parent = node.relPath.includes("/")
        ? node.relPath.slice(0, node.relPath.lastIndexOf("/"))
        : "";
      set({ selected: node, activeFolder: parent });
    }
  },

  setActiveFolder: (rel) => set({ activeFolder: rel }),

  saveSettings: async (s) => {
    try {
      await api.setSettings(s);
      set({ settings: s });
    } catch (e) {
      set({ error: String(e) });
    }
  },
}));
