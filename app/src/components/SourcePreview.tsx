import { useEffect, useRef, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { FileQuestion, ExternalLink, Loader2 } from "lucide-react";
import * as pdfjsLib from "pdfjs-dist";
import pdfWorkerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { renderAsync } from "docx-preview";
import * as XLSX from "@e965/xlsx";
import { api, assetUrl } from "../lib/ipc";
import type { FsNode } from "../lib/types";

pdfjsLib.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

type Cat = "image" | "audio" | "pdf" | "docx" | "excel" | "text" | "binary";

function categoryOf(kind: string): Cat {
  if (kind === "image") return "image";
  if (kind === "audio") return "audio";
  if (kind === "pdf") return "pdf";
  if (kind === "docx") return "docx";
  if (kind === "xlsx") return "excel"; // gồm cả xls/ods (FormatKind gộp về "xlsx")
  if (kind === "pptx") return "binary"; // webview chưa render được tốt -> mở ngoài
  return "text"; // csv, html, markdown, other
}

function Loading() {
  return (
    <div className="preview-loading">
      <Loader2 className="spin" size={22} />
      <span>Đang mở file…</span>
    </div>
  );
}

function PdfPreview({ relPath, onErr }: { relPath: string; onErr: (e: string) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const data = await api.readBytes(relPath);
        const pdf = await pdfjsLib.getDocument({ data }).promise;
        const cont = ref.current;
        if (cancelled || !cont) return;
        cont.innerHTML = "";
        const width = (cont.clientWidth || 800) - 8;
        for (let i = 1; i <= pdf.numPages; i++) {
          const page = await pdf.getPage(i);
          if (cancelled) return;
          const base = page.getViewport({ scale: 1 });
          const scale = Math.min(2, Math.max(0.6, width / base.width));
          const vp = page.getViewport({ scale });
          const canvas = document.createElement("canvas");
          canvas.width = vp.width;
          canvas.height = vp.height;
          canvas.className = "pdf-page";
          cont.appendChild(canvas);
          await page.render({ canvas, canvasContext: canvas.getContext("2d")!, viewport: vp })
            .promise;
        }
        if (!cancelled) setLoading(false);
      } catch (e) {
        onErr(String(e));
        setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [relPath, onErr]);
  return (
    <div className="preview pdf-canvas">
      <div ref={ref} className="pdf-pages" />
      {loading && <Loading />}
    </div>
  );
}

function DocxPreview({ relPath, onErr }: { relPath: string; onErr: (e: string) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const buf = await api.readBytes(relPath);
        const el = ref.current;
        if (cancelled || !el) return;
        el.innerHTML = "";
        await renderAsync(buf, el, undefined, {
          inWrapper: true,
          className: "docx",
          ignoreLastRenderedPageBreak: true,
        });
        if (!cancelled) setLoading(false);
      } catch (e) {
        onErr(String(e));
        setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [relPath, onErr]);
  return (
    <div className="preview docx-wrap">
      <div ref={ref} />
      {loading && <Loading />}
    </div>
  );
}

function ExcelPreview({ relPath, onErr }: { relPath: string; onErr: (e: string) => void }) {
  const [sheets, setSheets] = useState<{ name: string; html: string }[]>([]);
  const [active, setActive] = useState(0);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const buf = await api.readBytes(relPath);
        if (cancelled) return;
        const wb = XLSX.read(new Uint8Array(buf), { type: "array" });
        const s = wb.SheetNames.map((n) => ({
          name: n,
          html: XLSX.utils.sheet_to_html(wb.Sheets[n], { editable: false }),
        }));
        if (!cancelled) {
          setSheets(s);
          setActive(0);
          setLoading(false);
        }
      } catch (e) {
        onErr(String(e));
        setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [relPath, onErr]);
  if (loading) return <div className="preview"><Loading /></div>;
  if (!sheets.length) return <div className="preview"><p className="muted">Không có sheet.</p></div>;
  return (
    <div className="preview excel-wrap">
      {sheets.length > 1 && (
        <div className="sheet-tabs">
          {sheets.map((s, i) => (
            <button key={s.name} className={`seg ${i === active ? "on" : ""}`} onClick={() => setActive(i)}>
              {s.name}
            </button>
          ))}
        </div>
      )}
      <div className="excel-table" dangerouslySetInnerHTML={{ __html: sheets[active].html }} />
    </div>
  );
}

function BinaryFallback({ node, onError }: { node: FsNode; onError: (e: string) => void }) {
  async function openExternal() {
    try {
      await openPath(await api.resolvePath(node.relPath));
    } catch (e) {
      onError(String(e));
    }
  }
  return (
    <div className="preview center-preview">
      <FileQuestion size={40} className="muted" />
      <p>
        Chưa xem trước trực tiếp được <b>.{node.kind}</b> trong app.
      </p>
      <p className="muted">Đối chiếu bản Markdown bên phải, hoặc mở file gốc.</p>
      <button className="btn-ghost" onClick={openExternal}>
        <ExternalLink size={15} /> Mở file gốc
      </button>
    </div>
  );
}

export function SourcePreview({
  node,
  onError,
}: {
  node: FsNode;
  onError: (e: string) => void;
}) {
  const cat = categoryOf(node.kind);
  // image/audio dùng asset URL (img/audio src); pdf/docx/excel đọc bytes qua IPC.
  const [abs, setAbs] = useState<string | null>(null);
  const [text, setText] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setAbs(null);
    setText(null);
    if (cat === "text") {
      api
        .readTextFile(node.relPath)
        .then((t) => !cancelled && setText(t))
        .catch((e) => onError(String(e)));
    } else if (cat === "image" || cat === "audio") {
      api
        .resolvePath(node.relPath)
        .then((a) => !cancelled && setAbs(a))
        .catch((e) => onError(String(e)));
    }
    return () => {
      cancelled = true;
    };
  }, [node.relPath, cat, onError]);

  if (cat === "image")
    return <div className="preview image-preview">{abs && <img src={assetUrl(abs)} alt={node.name} />}</div>;

  if (cat === "audio")
    return (
      <div className="preview audio-preview">
        {abs && <audio controls src={assetUrl(abs)} />}
        <p className="muted">{node.name}</p>
      </div>
    );

  if (cat === "pdf") return <PdfPreview relPath={node.relPath} onErr={onError} />;
  if (cat === "docx") return <DocxPreview relPath={node.relPath} onErr={onError} />;
  if (cat === "excel") return <ExcelPreview relPath={node.relPath} onErr={onError} />;
  if (cat === "binary") return <BinaryFallback node={node} onError={onError} />;

  return (
    <div className="preview text-preview">
      {text !== null ? <pre>{text}</pre> : <Loading />}
    </div>
  );
}
