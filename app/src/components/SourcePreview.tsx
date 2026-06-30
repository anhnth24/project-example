import { useEffect, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { FileQuestion, ExternalLink } from "lucide-react";
import { api, assetUrl } from "../lib/ipc";
import type { FsNode } from "../lib/types";

type Cat = "image" | "audio" | "pdf" | "text" | "binary";

function categoryOf(kind: string): Cat {
  if (kind === "image") return "image";
  if (kind === "audio") return "audio";
  if (kind === "pdf") return "pdf";
  if (kind === "docx" || kind === "pptx" || kind === "xlsx") return "binary";
  return "text";
}

export function SourcePreview({
  node,
  onError,
}: {
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
        .resolvePath(node.relPath)
        .then((abs) => alive && setSrc(assetUrl(abs)))
        .catch((e) => onError(String(e)));
    } else if (cat === "text") {
      api
        .readTextFile(node.relPath)
        .then((t) => alive && setText(t))
        .catch((e) => onError(String(e)));
    }
    return () => {
      alive = false;
    };
  }, [node.relPath, cat, onError]);

  async function openExternal() {
    try {
      await openPath(await api.resolvePath(node.relPath));
    } catch (e) {
      onError(String(e));
    }
  }

  if (cat === "image")
    return <div className="preview image-preview">{src && <img src={src} alt={node.name} />}</div>;

  if (cat === "audio")
    return (
      <div className="preview audio-preview">
        {src && <audio controls src={src} />}
        <p className="muted">{node.name}</p>
      </div>
    );

  if (cat === "pdf")
    return (
      <div className="preview pdf-preview">
        {src && <iframe title={node.name} src={src} />}
        <div className="preview-note">
          PDF hiển thị tùy webview hệ điều hành. Nếu trống,{" "}
          <button className="link" onClick={openExternal}>
            mở bằng app ngoài
          </button>
          .
        </div>
      </div>
    );

  if (cat === "binary")
    return (
      <div className="preview center-preview">
        <FileQuestion size={40} className="muted" />
        <p>
          Không xem trước trực tiếp được <b>.{node.kind}</b> trong app.
        </p>
        <p className="muted">Đối chiếu bản Markdown bên phải, hoặc mở file gốc.</p>
        <button className="btn-ghost" onClick={openExternal}>
          <ExternalLink size={15} /> Mở file gốc
        </button>
      </div>
    );

  return (
    <div className="preview text-preview">
      {text !== null ? <pre>{text}</pre> : <div className="muted">Đang tải…</div>}
    </div>
  );
}
