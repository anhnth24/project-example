import { useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { api, assetUrl } from "../lib/ipc";
import type { FsNode } from "../lib/types";

type Cat = "image" | "audio" | "pdf" | "text" | "binary";

function categoryOf(kind: string): Cat {
  if (kind === "image") return "image";
  if (kind === "audio") return "audio";
  if (kind === "pdf") return "pdf";
  if (kind === "docx" || kind === "pptx" || kind === "xlsx") return "binary";
  // csv, html, markdown, other, txt...
  return "text";
}

export function SourcePreview({
  workspaceId,
  node,
  onError,
}: {
  workspaceId: string;
  node: FsNode;
  onError: (e: string) => void;
}) {
  const cat = categoryOf(node.kind);
  const [src, setSrc] = useState<string | null>(null);
  const [text, setText] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    setSrc(null);
    setText(null);
    if (cat === "image" || cat === "audio" || cat === "pdf") {
      api
        .resolvePath(workspaceId, node.relPath)
        .then((abs) => alive && setSrc(assetUrl(abs)))
        .catch((e) => onError(String(e)));
    } else if (cat === "text") {
      api
        .readTextFile(workspaceId, node.relPath)
        .then((t) => alive && setText(t))
        .catch((e) => onError(String(e)));
    }
    return () => {
      alive = false;
    };
  }, [workspaceId, node.relPath, cat, onError]);

  async function openExternal() {
    try {
      const abs = await api.resolvePath(workspaceId, node.relPath);
      await openPath(abs);
    } catch (e) {
      onError(String(e));
    }
  }

  if (cat === "image") {
    return (
      <div className="preview image-preview">
        {src && <img src={src} alt={node.name} />}
      </div>
    );
  }

  if (cat === "audio") {
    return (
      <div className="preview audio-preview">
        {src && <audio controls src={src} />}
        <p className="muted">{node.name}</p>
      </div>
    );
  }

  if (cat === "pdf") {
    return (
      <div className="preview pdf-preview">
        {src && <iframe title={node.name} src={src} />}
        <div className="preview-note">
          PDF hiển thị tùy webview của hệ điều hành. Nếu trống,{" "}
          <button className="link-btn" onClick={openExternal}>
            mở bằng app ngoài
          </button>
          .
        </div>
      </div>
    );
  }

  if (cat === "binary") {
    return (
      <div className="preview binary-preview">
        <p>Không xem trước trực tiếp được định dạng <b>{node.kind}</b> trong app.</p>
        <p className="muted">
          Hãy đối chiếu với bản Markdown bên phải, hoặc mở file gốc bằng ứng dụng mặc định.
        </p>
        <button onClick={openExternal}>Mở file gốc</button>
      </div>
    );
  }

  // text
  return (
    <div className="preview text-preview">
      {text !== null ? <pre>{text}</pre> : <div className="muted">Đang tải…</div>}
    </div>
  );
}
