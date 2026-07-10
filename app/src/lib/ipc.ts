// Lớp bọc duy nhất gọi sang backend Rust. Tauri tự đổi key camelCase (JS) -> snake_case (Rust).
import { invoke } from "@tauri-apps/api/core";
import type { FsNode, Settings } from "./types";

export const api = {
  supportedExtensions: () => invoke<string[]>("supported_extensions"),

  getDataRoot: () => invoke<string>("get_data_root"),
  setDataRoot: (path: string) => invoke<string>("set_data_root", { path }),

  readTree: () => invoke<FsNode>("read_tree"),

  createFolder: (parentRel: string, name: string) =>
    invoke<void>("create_folder", { parentRel, name }),
  createMarkdown: (parentRel: string, name: string) =>
    invoke<FsNode>("create_markdown", { parentRel, name }),
  renameNode: (relPath: string, newName: string) =>
    invoke<void>("rename_node", { relPath, newName }),
  deleteNode: (relPath: string) => invoke<void>("delete_node", { relPath }),

  importFile: (folderRel: string, sourceAbs: string) =>
    invoke<FsNode>("import_file", { folderRel, sourceAbs }),
  importFileOnly: (folderRel: string, sourceAbs: string) =>
    invoke<FsNode>("import_file_only", { folderRel, sourceAbs }),
  reconvert: (sourceRel: string) => invoke<string>("reconvert", { sourceRel }),

  readTextFile: (relPath: string) => invoke<string>("read_text_file", { relPath }),
  writeTextFile: (relPath: string, content: string) =>
    invoke<void>("write_text_file", { relPath, content }),
  readTextPreview: (relPath: string, maxBytes: number) =>
    invoke<{ text: string; truncated: boolean; size: number }>("read_text_preview", {
      relPath,
      maxBytes,
    }),
  fileSize: (relPath: string) => invoke<number>("file_size", { relPath }),

  resolvePath: (relPath: string) => invoke<string>("resolve_path", { relPath }),
  /** Bytes thô của file (ArrayBuffer) cho pdf.js/docx-preview/SheetJS. */
  readBytes: (relPath: string) => invoke<ArrayBuffer>("read_bytes", { relPath }),

  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
};
