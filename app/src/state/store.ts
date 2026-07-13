import { create } from "zustand";
import { check as checkForUpdate } from "@tauri-apps/plugin-updater";
import { api } from "../lib/ipc";
import { findByRel, parentRel, isWithinRel } from "../lib/tree";
import type {
  AppView,
  ConvertJob,
  DocumentMode,
  DocumentSession,
  FsNode,
  MarkdownTab,
  Project,
  Settings,
} from "../lib/types";

const loadTokens = new Map<string, number>();

function baselineKey(dataRoot: string, relPath: string): string {
  return `markhand:baseline:${dataRoot}:${relPath}`;
}

function readBaseline(dataRoot: string, relPath: string, fallback: string): string {
  try {
    return localStorage.getItem(baselineKey(dataRoot, relPath)) ?? fallback;
  } catch {
    return fallback;
  }
}

function persistBaseline(dataRoot: string, relPath: string, markdown: string): void {
  try {
    localStorage.setItem(baselineKey(dataRoot, relPath), markdown);
  } catch {
    // Local storage is a convenience cache only. Markdown remains on disk.
  }
}

function defaultSession(relPath: string): DocumentSession {
  return {
    relPath,
    baseline: "",
    savedDraft: "",
    draft: "",
    loaded: false,
    dirty: false,
    saving: false,
    revision: 0,
    mode: "compare",
    markdownTab: "preview",
    savedAt: null,
  };
}

interface AppStore {
  dataRoot: string;
  tree: FsNode | null;
  activeFolder: string;
  settings: Settings | null;
  supportedExts: string[];
  projects: Project[];
  activeProjectId: string | null;
  error: string | null;

  view: AppView;
  openTabs: string[];
  activeTab: string | null;
  sessions: Record<string, DocumentSession>;
  intelligenceScope: string[];

  jobs: ConvertJob[];
  queueRunning: boolean;
  activeImports: number;
  workspaceChanging: boolean;

  init: () => Promise<void>;
  setError: (error: string | null) => void;
  refreshTree: () => Promise<void>;
  refreshProjects: () => Promise<void>;
  setView: (view: AppView) => void;
  setActiveProject: (projectId: string) => void;
  setIntelligenceScope: (sourceRels: string[]) => void;
  openNode: (node: FsNode) => void;
  closeTab: (relPath: string) => void;
  closeTabsWithin: (relPath: string) => void;
  setActiveFolder: (relPath: string) => void;

  loadSession: (relPath: string, conversionBaseline?: boolean) => Promise<void>;
  updateDraft: (relPath: string, draft: string) => void;
  setDocumentMode: (relPath: string, mode: DocumentMode) => void;
  setMarkdownTab: (relPath: string, tab: MarkdownTab) => void;
  saveSession: (relPath?: string) => Promise<void>;

  changeDataRoot: (path: string) => Promise<void>;
  saveSettings: (settings: Settings) => Promise<void>;
  importSources: (sourcePaths: string[]) => Promise<void>;
  enqueueConversions: (relPaths: string[]) => void;
  runQueue: () => Promise<void>;
  retryJob: (id: string) => void;
  clearFinishedJobs: () => void;
}

export const useStore = create<AppStore>((set, get) => ({
  dataRoot: "",
  tree: null,
  activeFolder: "",
  settings: null,
  supportedExts: [],
  projects: [],
  activeProjectId: null,
  error: null,

  view: "home",
  openTabs: [],
  activeTab: null,
  sessions: {},
  intelligenceScope: [],

  jobs: [],
  queueRunning: false,
  activeImports: 0,
  workspaceChanging: false,

  setError: (error) => set({ error }),

  init: async () => {
    try {
      const [supportedExts, settings, dataRoot, projects] = await Promise.all([
        api.supportedExtensions(),
        api.getSettings(),
        api.getDataRoot(),
        api.listProjects(),
      ]);
      const storedProject = localStorage.getItem(`markhand:project:${dataRoot}`);
      const activeProject =
        projects.find((project) => project.id === storedProject) ?? projects[0] ?? null;
      set({
        supportedExts,
        settings,
        dataRoot,
        projects,
        activeProjectId: activeProject?.id ?? null,
        activeFolder: activeProject?.rootRel ?? "",
      });
      await get().refreshTree();
      if (settings.autoCheckUpdate) {
        // Chạy nền, không chặn init(); lỗi mạng/offline thì im lặng bỏ qua.
        checkForUpdate()
          .then((update) => {
            if (update) {
              set({ error: `Có bản ${update.version} mới hơn — mở Cài đặt để cập nhật.` });
            }
          })
          .catch(() => {});
      }
    } catch (error) {
      set({ error: String(error) });
    }
  },

  refreshTree: async () => {
    try {
      const tree = await api.readTree();
      set((state) => {
        const openTabs = state.openTabs.filter((relPath) => findByRel(tree, relPath));
        const activeTab =
          state.activeTab && openTabs.includes(state.activeTab)
            ? state.activeTab
            : (openTabs[openTabs.length - 1] ?? null);
        return {
          tree,
          openTabs,
          activeTab,
          view: state.view === "document" && !activeTab ? "home" : state.view,
        };
      });
    } catch (error) {
      set({ error: String(error) });
    }
  },

  refreshProjects: async () => {
    try {
      const projects = await api.listProjects();
      set((state) => {
        const active =
          projects.find((project) => project.id === state.activeProjectId) ??
          projects[0] ??
          null;
        return {
          projects,
          activeProjectId: active?.id ?? null,
          activeFolder: active
            ? active.rootRel === "" ||
              state.activeFolder === active.rootRel ||
              state.activeFolder.startsWith(`${active.rootRel}/`)
              ? state.activeFolder
              : active.rootRel
            : "",
        };
      });
    } catch (error) {
      set({ error: String(error) });
    }
  },

  setView: (view) => set({ view }),
  setActiveProject: (projectId) => {
    const project = get().projects.find((candidate) => candidate.id === projectId);
    if (!project) return;
    localStorage.setItem(`markhand:project:${get().dataRoot}`, project.id);
    set({
      activeProjectId: project.id,
      activeFolder: project.rootRel,
      view: "home",
      intelligenceScope: [],
    });
  },
  setIntelligenceScope: (intelligenceScope) => set({ intelligenceScope }),

  openNode: (node) => {
    if (node.isDir) {
      set({ activeFolder: node.relPath });
      return;
    }
    set((state) => ({
      activeFolder: parentRel(node.relPath),
      activeTab: node.relPath,
      openTabs: state.openTabs.includes(node.relPath)
        ? state.openTabs
        : [...state.openTabs, node.relPath],
      sessions: state.sessions[node.relPath]
        ? state.sessions
        : { ...state.sessions, [node.relPath]: defaultSession(node.relPath) },
      view: "document",
    }));
    void get().loadSession(node.relPath);
  },

  closeTab: (relPath) => {
    set((state) => {
      const index = state.openTabs.indexOf(relPath);
      const openTabs = state.openTabs.filter((tab) => tab !== relPath);
      let activeTab = state.activeTab;
      if (activeTab === relPath) {
        activeTab = openTabs[Math.min(index, openTabs.length - 1)] ?? null;
      }
      const sessions = { ...state.sessions };
      delete sessions[relPath];
      return {
        openTabs,
        activeTab,
        sessions,
        view: activeTab ? state.view : "home",
      };
    });
  },

  closeTabsWithin: (relPath) => {
    set((state) => {
      const removed = state.openTabs.filter((tab) => isWithinRel(tab, relPath));
      const openTabs = state.openTabs.filter((tab) => !removed.includes(tab));
      const sessions = { ...state.sessions };
      removed.forEach((tab) => delete sessions[tab]);
      const activeTab =
        state.activeTab && removed.includes(state.activeTab)
          ? (openTabs[openTabs.length - 1] ?? null)
          : state.activeTab;
      return {
        openTabs,
        activeTab,
        sessions,
        view: activeTab ? state.view : "home",
      };
    });
  },

  setActiveFolder: (activeFolder) => set({ activeFolder }),

  loadSession: async (relPath, conversionBaseline = false) => {
    const existing = get().sessions[relPath];
    if (existing?.loaded && !conversionBaseline) return;
    const token = (loadTokens.get(relPath) ?? 0) + 1;
    loadTokens.set(relPath, token);
    const node = findByRel(get().tree, relPath);
    if (!node) return;
    if (!node.mdRelPath) {
      set((state) => ({
        sessions: {
          ...state.sessions,
          [relPath]: {
            ...(state.sessions[relPath] ?? defaultSession(relPath)),
            loaded: true,
            baseline: "",
            savedDraft: "",
            draft: "",
            dirty: false,
            saving: false,
            revision: 0,
          },
        },
      }));
      return;
    }
    try {
      const markdown = await api.readTextFile(node.mdRelPath);
      const baseline = conversionBaseline
        ? markdown
        : readBaseline(get().dataRoot, relPath, markdown);
      persistBaseline(get().dataRoot, relPath, baseline);
      set((state) => {
        if (loadTokens.get(relPath) !== token || !state.sessions[relPath]) {
          return state;
        }
        return {
          sessions: {
            ...state.sessions,
            [relPath]: {
              ...state.sessions[relPath],
              baseline,
              savedDraft: markdown,
              draft: markdown,
              loaded: true,
              dirty: false,
              saving: false,
              revision: 0,
              savedAt: conversionBaseline
                ? new Date().toLocaleTimeString("vi-VN", {
                    hour: "2-digit",
                    minute: "2-digit",
                  })
                : state.sessions[relPath].savedAt,
            },
          },
        };
      });
    } catch (error) {
      if (loadTokens.get(relPath) === token) set({ error: String(error) });
    }
  },

  updateDraft: (relPath, draft) =>
    set((state) => {
      const session = state.sessions[relPath] ?? defaultSession(relPath);
      if (
        state.jobs.some(
          (job) =>
            job.relPath === relPath &&
            (job.status === "queued" || job.status === "running"),
        )
      ) {
        return state;
      }
      return {
        sessions: {
          ...state.sessions,
          [relPath]: {
            ...session,
            draft,
            dirty: draft !== session.savedDraft,
            revision: session.revision + 1,
          },
        },
      };
    }),

  setDocumentMode: (relPath, mode) =>
    set((state) => {
      const session = state.sessions[relPath] ?? defaultSession(relPath);
      return {
        sessions: {
          ...state.sessions,
          [relPath]: { ...session, mode },
        },
      };
    }),

  setMarkdownTab: (relPath, markdownTab) =>
    set((state) => {
      const session = state.sessions[relPath] ?? defaultSession(relPath);
      return {
        sessions: {
          ...state.sessions,
          [relPath]: { ...session, markdownTab },
        },
      };
    }),

  saveSession: async (requestedRelPath) => {
    const relPath = requestedRelPath ?? get().activeTab;
    if (!relPath) return;
    const session = get().sessions[relPath];
    const node = findByRel(get().tree, relPath);
    if (!session?.dirty || session.saving || !node?.mdRelPath) return;
    const writtenDraft = session.draft;
    set((state) => ({
      sessions: {
        ...state.sessions,
        [relPath]: { ...state.sessions[relPath], saving: true },
      },
    }));
    try {
      await api.writeTextFile(node.mdRelPath, writtenDraft);
      set((state) => {
        const current = state.sessions[relPath];
        if (!current) return state;
        return {
          sessions: {
            ...state.sessions,
            [relPath]: {
              ...current,
              savedDraft: writtenDraft,
              dirty: current.draft !== writtenDraft,
              saving: false,
              savedAt: new Date().toLocaleTimeString("vi-VN", {
                hour: "2-digit",
                minute: "2-digit",
              }),
            },
          },
        };
      });
    } catch (error) {
      set((state) => ({
        error: String(error),
        sessions: state.sessions[relPath]
          ? {
              ...state.sessions,
              [relPath]: { ...state.sessions[relPath], saving: false },
            }
          : state.sessions,
      }));
    }
  },

  changeDataRoot: async (path) => {
    if (
      get().activeImports > 0 ||
      get().jobs.some((job) => job.status === "queued" || job.status === "running")
    ) {
      set({ error: "Hãy chờ import và hàng đợi convert hoàn tất trước khi đổi thư mục DATA." });
      return;
    }
    set({ workspaceChanging: true });
    try {
      const dataRoot = await api.setDataRoot(path);
      set({
        dataRoot,
        tree: null,
        activeFolder: "",
        view: "home",
        openTabs: [],
        activeTab: null,
        sessions: {},
        intelligenceScope: [],
        projects: [],
        activeProjectId: null,
        jobs: [],
      });
      await get().refreshProjects();
      await get().refreshTree();
    } catch (error) {
      set({ error: String(error) });
    } finally {
      set({ workspaceChanging: false });
    }
  },

  saveSettings: async (settings) => {
    try {
      await api.setSettings(settings);
      set({ settings });
    } catch (error) {
      set({ error: String(error) });
      throw error;
    }
  },

  importSources: async (sourcePaths) => {
    if (!sourcePaths.length) return;
    if (get().workspaceChanging) {
      set({ error: "Đang đổi thư mục DATA; hãy thử import lại sau." });
      return;
    }
    set((state) => ({ activeImports: state.activeImports + 1 }));
    const folder = get().activeFolder;
    const imported: string[] = [];
    const errors: string[] = [];
    try {
      for (const sourcePath of sourcePaths) {
        try {
          const node = await api.importFileOnly(folder, sourcePath);
          imported.push(node.relPath);
        } catch (error) {
          errors.push(String(error));
        }
      }
      await get().refreshTree();
      if (imported.length) get().enqueueConversions(imported);
      if (errors.length) set({ error: errors.join(" • ") });
    } finally {
      set((state) => ({ activeImports: Math.max(0, state.activeImports - 1) }));
    }
  },

  enqueueConversions: (relPaths) => {
    const state = get();
    const jobs: ConvertJob[] = [];
    for (const relPath of relPaths) {
      const node = findByRel(state.tree, relPath);
      if (!node || node.isDir || !node.supported) continue;
      if (state.sessions[relPath]?.dirty) {
        set({ error: `Hãy lưu thay đổi trong “${node.name}” trước khi convert lại.` });
        continue;
      }
      const duplicate = state.jobs.some(
        (job) =>
          job.relPath === relPath &&
          (job.status === "queued" || job.status === "running"),
      );
      if (duplicate) continue;
      jobs.push({
        id: `${Date.now()}-${jobs.length}-${relPath}`,
        relPath,
        name: node.name,
        kind: node.kind,
        status: "queued",
        error: null,
        queuedAt: Date.now(),
      });
    }
    if (!jobs.length) return;
    set((current) => ({ jobs: [...current.jobs, ...jobs] }));
    void get().runQueue();
  },

  runQueue: async () => {
    if (get().queueRunning) return;
    set({ queueRunning: true });
    try {
      while (true) {
        const next = get().jobs.find((job) => job.status === "queued");
        if (!next) break;
        const startingSession = get().sessions[next.relPath];
        if (startingSession?.dirty || startingSession?.saving) {
          set((state) => ({
            jobs: state.jobs.map((job) =>
              job.id === next.id
                ? {
                    ...job,
                    status: "error",
                    error: "Tài liệu có thay đổi chưa lưu; job đã được dừng.",
                  }
                : job,
            ),
          }));
          continue;
        }
        const startingRevision = startingSession?.revision;
        set((state) => ({
          jobs: state.jobs.map((job) =>
            job.id === next.id ? { ...job, status: "running", error: null } : job,
          ),
        }));
        try {
          await api.reconvert(next.relPath);
          await get().refreshTree();
          const currentSession = get().sessions[next.relPath];
          if (
            currentSession &&
            !currentSession.dirty &&
            (startingRevision === undefined ||
              currentSession.revision === startingRevision)
          ) {
            await get().loadSession(next.relPath, true);
          }
          try {
            await api.rebuildKnowledgeIndex([next.relPath]);
          } catch (indexError) {
            set({ error: `Đã convert nhưng index lỗi: ${String(indexError)}` });
          }
          set((state) => ({
            jobs: state.jobs.map((job) =>
              job.id === next.id ? { ...job, status: "done", error: null } : job,
            ),
          }));
        } catch (error) {
          set((state) => ({
            jobs: state.jobs.map((job) =>
              job.id === next.id
                ? { ...job, status: "error", error: String(error) }
                : job,
            ),
          }));
        }
      }
    } finally {
      set({ queueRunning: false });
      // A job can be enqueued between the final lookup and this state update.
      if (get().jobs.some((job) => job.status === "queued")) {
        void get().runQueue();
      }
    }
  },

  retryJob: (id) => {
    set((state) => ({
      jobs: state.jobs.map((job) =>
        job.id === id ? { ...job, status: "queued", error: null } : job,
      ),
    }));
    void get().runQueue();
  },

  clearFinishedJobs: () =>
    set((state) => ({
      jobs: state.jobs.filter(
        (job) => job.status === "queued" || job.status === "running",
      ),
    })),
}));
