import { create } from "zustand";
import { api } from "../lib/ipc";
import type { FsNode, Settings } from "../lib/types";

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
  dataRoot: string;
  tree: FsNode | null;
  selected: FsNode | null;
  /** Thư mục đích cho thao tác tạo/upload (relPath; "" = gốc DATA). */
  activeFolder: string;
  settings: Settings | null;
  supportedExts: string[];
  error: string | null;

  init: () => Promise<void>;
  setError: (e: string | null) => void;
  refreshTree: () => Promise<void>;
  selectNode: (node: FsNode) => void;
  setActiveFolder: (rel: string) => void;
  changeDataRoot: (path: string) => Promise<void>;
  saveSettings: (s: Settings) => Promise<void>;
}

export const useStore = create<AppStore>((set, get) => ({
  dataRoot: "",
  tree: null,
  selected: null,
  activeFolder: "",
  settings: null,
  supportedExts: [],
  error: null,

  setError: (e) => set({ error: e }),

  init: async () => {
    try {
      const [exts, settings, dataRoot] = await Promise.all([
        api.supportedExtensions(),
        api.getSettings(),
        api.getDataRoot(),
      ]);
      set({ supportedExts: exts, settings, dataRoot });
      await get().refreshTree();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  refreshTree: async () => {
    try {
      const tree = await api.readTree();
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
      const parent = node.relPath.includes("/")
        ? node.relPath.slice(0, node.relPath.lastIndexOf("/"))
        : "";
      set({ selected: node, activeFolder: parent });
    }
  },

  setActiveFolder: (rel) => set({ activeFolder: rel }),

  changeDataRoot: async (path) => {
    try {
      const root = await api.setDataRoot(path);
      set({ dataRoot: root, selected: null, activeFolder: "" });
      await get().refreshTree();
    } catch (e) {
      set({ error: String(e) });
    }
  },

  saveSettings: async (s) => {
    try {
      await api.setSettings(s);
      set({ settings: s });
    } catch (e) {
      set({ error: String(e) });
    }
  },
}));
