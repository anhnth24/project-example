// Lớp bọc duy nhất gọi sang backend Rust. Tauri tự đổi key camelCase (JS) -> snake_case (Rust).
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import type { FsNode, Settings, Workspace } from "./types";

export const api = {
  supportedExtensions: () => invoke<string[]>("supported_extensions"),

  listWorkspaces: () => invoke<Workspace[]>("list_workspaces"),
  addWorkspace: (path: string, name?: string) =>
    invoke<Workspace>("add_workspace", { path, name }),
  removeWorkspace: (id: string) => invoke<void>("remove_workspace", { id }),

  readTree: (workspaceId: string) => invoke<FsNode>("read_tree", { workspaceId }),

  createFolder: (workspaceId: string, parentRel: string, name: string) =>
    invoke<void>("create_folder", { workspaceId, parentRel, name }),
  createMarkdown: (workspaceId: string, parentRel: string, name: string) =>
    invoke<FsNode>("create_markdown", { workspaceId, parentRel, name }),
  renameNode: (workspaceId: string, relPath: string, newName: string) =>
    invoke<void>("rename_node", { workspaceId, relPath, newName }),
  deleteNode: (workspaceId: string, relPath: string) =>
    invoke<void>("delete_node", { workspaceId, relPath }),

  importFile: (workspaceId: string, folderRel: string, sourceAbs: string) =>
    invoke<FsNode>("import_file", { workspaceId, folderRel, sourceAbs }),
  reconvert: (workspaceId: string, sourceRel: string) =>
    invoke<string>("reconvert", { workspaceId, sourceRel }),

  readTextFile: (workspaceId: string, relPath: string) =>
    invoke<string>("read_text_file", { workspaceId, relPath }),
  writeTextFile: (workspaceId: string, relPath: string, content: string) =>
    invoke<void>("write_text_file", { workspaceId, relPath, content }),

  resolvePath: (workspaceId: string, relPath: string) =>
    invoke<string>("resolve_path", { workspaceId, relPath }),

  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
};

/** Tạo URL asset (asset://) để hiển thị ảnh/pdf/audio từ đường dẫn tuyệt đối. */
export const assetUrl = (absPath: string) => convertFileSrc(absPath);
