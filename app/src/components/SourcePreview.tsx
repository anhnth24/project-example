import { useEffect, useRef, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { FileQuestion, ExternalLink, Loader2, FileWarning } from "lucide-react";
import * as pdfjsLib from "pdfjs-dist";
import pdfWorkerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import { renderAsync } from "docx-preview";
import * as XLSX from "@e965/xlsx";
import { api } from "../lib/ipc";
import type { FsNode } from "../lib/types";
import { Button, Notice } from "./ui";
import { PptxPreview } from "./PptxPreview";

pdfjsLib.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

const KB = 1024;
const MB = 1024 * 1024;
const TEXT_CAP = 512 * KB; // chỉ đọc 512KB đầu cho text/csv/log
const XLSX_ROW_CAP = 1000; // tối đa 1000 dòng/sheet khi preview
const LIMIT: Record<string, number> = {
  image: 60 * MB,
  audio: 120 * MB,
  pdf: 80 * MB,
  docx: 40 * MB,
  excel: 40 * MB,
};

type Cat = "image" | "audio" | "pdf" | "docx" | "excel" | "pptx" | "text" | "binary";

function categoryOf(kind: string): Cat {
  if (kind === "image") return "image";
  if (kind === "audio") return "audio";
  if (kind === "pdf") return "pdf";
  if (kind === "docx") return "docx";
  if (kind === "xlsx") return "excel";
  if (kind === "pptx") return "pptx";
  return "text";
}

function humanSize(b: number): string {
  if (b < KB) return `${b} B`;
  if (b < MB) return `${(b / KB).toFixed(0)} KB`;
  if (b < 1024 * MB) return `${(b / MB).toFixed(1)} MB`;
  return `${(b / (1024 * MB)).toFixed(2)} GB`;
}

function mimeOf(name: string): string {
  const e = name.toLowerCase().split(".").pop() || "";
  const m: Record<string, string> = {
    jpg: "image/jpeg", jpeg: "image/jpeg", png: "image/png", gif: "image/gif",
    webp: "image/webp", bmp: "image/bmp", tif: "image/tiff", tiff: "image/tiff",
    svg: "image/svg+xml", mp3: "audio/mpeg", wav: "audio/wav", ogg: "audio/ogg",
    m4a: "audio/mp4", flac: "audio/flac",
  };
  return m[e] || "application/octet-stream";
}

async function openExternal(relPath: string, onErr: (e: string) => void) {
  try {
    await openPath(await api.resolvePath(relPath));
  } catch (e) {
    onErr(String(e));
  }
}

function Loading() {
  return (
    <div className="preview-loading">
      <Loader2 className="spin" size={22} />
      <span>Đang mở file…</span>
    </div>
  );
}

/** Cổng kích thước: file lớn -> hỏi trước khi render trong app. */
function useSizeGate(relPath: string, limit: number) {
  const [size, setSize] = useState<number | null>(null);
  const [forced, setForced] = useState(false);
  useEffect(() => {
    let c = false;
    setSize(null);
    setForced(false);
    api.fileSize(relPath).then((s) => !c && setSize(s)).catch(() => !c && setSize(0));
    return () => {
      c = true;
    };
  }, [relPath]);
  return {
    ready: size !== null,
    size: size ?? 0,
    tooBig: size !== null && size > limit && !forced,
    force: () => setForced(true),
  };
}

function BigGuard({
  size,
  onForce,
  relPath,
  onErr,
}: {
  size: number;
  onForce: () => void;
  relPath: string;
  onErr: (e: string) => void;
}) {
  return (
    <div className="preview center-preview">
      <FileWarning size={40} className="muted" />
      <p>
        File khá lớn (<b>{humanSize(size)}</b>). Render trong app có thể chậm hoặc tốn bộ nhớ.
      </p>
      <div className="guard-actions">
        <Button variant="ghost" onClick={onForce}>
          Vẫn xem trong app
        </Button>
        <Button
          variant="primary"
          icon={<ExternalLink size={15} />}
          onClick={() => openExternal(relPath, onErr)}
        >
          Mở bằng app ngoài
        </Button>
      </div>
    </div>
  );
}

function BlobMedia({
  relPath,
  name,
  kind,
  onErr,
}: {
  relPath: string;
  name: string;
  kind: "image" | "audio";
  onErr: (e: string) => void;
}) {
  const gate = useSizeGate(relPath, LIMIT[kind]);
  const [url, setUrl] = useState<string | null>(null);
  const show = gate.ready && !gate.tooBig;
  useEffect(() => {
    if (!show) return;
    let c = false;
    let obj: string | undefined;
    (async () => {
      const buf = await api.readBytes(relPath);
      if (c) return;
      obj = URL.createObjectURL(new Blob([buf], { type: mimeOf(name) }));
      setUrl(obj);
    })().catch((e) => onErr(String(e)));
    return () => {
      c = true;
      if (obj) URL.revokeObjectURL(obj);
    };
  }, [relPath, name, show, onErr]);

  if (!gate.ready) return <div className="preview"><Loading /></div>;
  if (gate.tooBig) return <BigGuard size={gate.size} onForce={gate.force} relPath={relPath} onErr={onErr} />;
  if (!url) return <div className="preview"><Loading /></div>;
  return kind === "image" ? (
    <div className="preview image-preview">
      <img src={url} alt={name} />
    </div>
  ) : (
    <div className="preview audio-preview">
      <audio controls src={url} />
      <p className="muted">{name}</p>
    </div>
  );
}

function TextPreview({ relPath, onErr }: { relPath: string; onErr: (e: string) => void }) {
  const [data, setData] = useState<{ text: string; truncated: boolean; size: number } | null>(null);
  useEffect(() => {
    let c = false;
    setData(null);
    api.readTextPreview(relPath, TEXT_CAP).then((d) => !c && setData(d)).catch((e) => onErr(String(e)));
    return () => {
      c = true;
    };
  }, [relPath, onErr]);
  if (!data) return <div className="preview"><Loading /></div>;
  return (
    <div className="preview text-preview">
      {data.truncated && (
        <div className="preview-banner-wrap">
          <Notice
            tone="warning"
            action={
              <Button
                variant="ghost"
                size="sm"
                onClick={() => openExternal(relPath, onErr)}
              >
                Mở ngoài
              </Button>
            }
          >
            File lớn ({humanSize(data.size)}) — chỉ hiển thị {humanSize(TEXT_CAP)} đầu.
          </Notice>
        </div>
      )}
      <pre>{data.text}</pre>
    </div>
  );
}

function PdfPreview({ relPath, onErr }: { relPath: string; onErr: (e: string) => void }) {
  const gate = useSizeGate(relPath, LIMIT.pdf);
  const scrollRef = useRef<HTMLDivElement>(null);
  const pagesRef = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(true);
  const show = gate.ready && !gate.tooBig;

  useEffect(() => {
    if (!show) return;
    let cancelled = false;
    let observer: IntersectionObserver | null = null;
    setLoading(true);
    (async () => {
      const data = await api.readBytes(relPath);
      const pdf = await pdfjsLib.getDocument({ data }).promise;
      const scroll = scrollRef.current;
      const pages = pagesRef.current;
      if (cancelled || !scroll || !pages) return;
      pages.innerHTML = "";
      const width = (scroll.clientWidth || 800) - 40;
      const p1 = await pdf.getPage(1);
      const b1 = p1.getViewport({ scale: 1 });
      const fit = Math.min(2, Math.max(0.6, width / b1.width));
      const estH = Math.round(b1.height * fit);
      const done = new Set<number>();
      observer = new IntersectionObserver(
        (entries) => {
          for (const e of entries) {
            if (!e.isIntersecting) continue;
            const div = e.target as HTMLDivElement;
            const n = Number(div.dataset.page);
            if (done.has(n)) continue;
            done.add(n);
            observer!.unobserve(div);
            pdf.getPage(n).then(async (page) => {
              if (cancelled) return;
              // render theo devicePixelRatio để chữ sắc nét trên màn scale >100%
              const dpr = window.devicePixelRatio || 1;
              const vp = page.getViewport({ scale: fit * dpr });
              const canvas = document.createElement("canvas");
              canvas.width = vp.width;
              canvas.height = vp.height;
              canvas.style.width = `${Math.round(vp.width / dpr)}px`;
              canvas.className = "pdf-page";
              div.style.height = "auto";
              div.innerHTML = "";
              div.appendChild(canvas);
              await page.render({ canvas, canvasContext: canvas.getContext("2d")!, viewport: vp }).promise;
            });
          }
        },
        { root: scroll, rootMargin: "800px 0px" }
      );
      // chỉ render trang vào tầm nhìn -> mở PDF nhiều trang vẫn nhẹ.
      for (let i = 1; i <= pdf.numPages; i++) {
        const ph = document.createElement("div");
        ph.className = "pdf-ph";
        ph.dataset.page = String(i);
        ph.style.height = `${estH}px`;
        pages.appendChild(ph);
        observer.observe(ph);
      }
      if (!cancelled) setLoading(false);
    })().catch((e) => {
      onErr(String(e));
      setLoading(false);
    });
    return () => {
      cancelled = true;
      observer?.disconnect();
    };
  }, [relPath, show, onErr]);

  if (!gate.ready) return <div className="preview"><Loading /></div>;
  if (gate.tooBig) return <BigGuard size={gate.size} onForce={gate.force} relPath={relPath} onErr={onErr} />;
  return (
    <div className="preview pdf-canvas" ref={scrollRef}>
      <div ref={pagesRef} className="pdf-pages" />
      {loading && <Loading />}
    </div>
  );
}

function DocxPreview({ relPath, onErr }: { relPath: string; onErr: (e: string) => void }) {
  const gate = useSizeGate(relPath, LIMIT.docx);
  const ref = useRef<HTMLDivElement>(null);
  const [loading, setLoading] = useState(true);
  const show = gate.ready && !gate.tooBig;
  useEffect(() => {
    if (!show) return;
    let cancelled = false;
    setLoading(true);
    (async () => {
      const buf = await api.readBytes(relPath);
      const el = ref.current;
      if (cancelled || !el) return;
      el.innerHTML = "";
      await renderAsync(buf, el, undefined, {
        inWrapper: true,
        className: "docx",
        ignoreLastRenderedPageBreak: true,
        // Cho nội dung co theo bề rộng khung (tránh trang A4 cố định bị cắt mất chữ).
        ignoreWidth: true,
        ignoreHeight: true,
        breakPages: false,
        experimental: true,
      });
      if (!cancelled) setLoading(false);
    })().catch((e) => {
      onErr(String(e));
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [relPath, show, onErr]);
  if (!gate.ready) return <div className="preview"><Loading /></div>;
  if (gate.tooBig) return <BigGuard size={gate.size} onForce={gate.force} relPath={relPath} onErr={onErr} />;
  return (
    <div className="preview docx-wrap">
      <div ref={ref} />
      {loading && <Loading />}
    </div>
  );
}

function ExcelPreview({ relPath, onErr }: { relPath: string; onErr: (e: string) => void }) {
  const gate = useSizeGate(relPath, LIMIT.excel);
  const [sheets, setSheets] = useState<{ name: string; html: string; capped: number }[]>([]);
  const [active, setActive] = useState(0);
  const [loading, setLoading] = useState(true);
  const show = gate.ready && !gate.tooBig;
  useEffect(() => {
    if (!show) return;
    let cancelled = false;
    setLoading(true);
    (async () => {
      const buf = await api.readBytes(relPath);
      if (cancelled) return;
      const wb = XLSX.read(new Uint8Array(buf), { type: "array" });
      const s = wb.SheetNames.map((n) => {
        const ws = wb.Sheets[n];
        let capped = 0;
        const ref = ws["!ref"];
        if (ref) {
          const range = XLSX.utils.decode_range(ref);
          const rows = range.e.r - range.s.r + 1;
          if (rows > XLSX_ROW_CAP) {
            capped = rows;
            range.e.r = range.s.r + XLSX_ROW_CAP - 1;
            ws["!ref"] = XLSX.utils.encode_range(range);
          }
        }
        return {
          name: n,
          html: XLSX.utils.sheet_to_html(ws, {
            editable: false,
            header: "",
            footer: "",
          }),
          capped,
        };
      });
      if (!cancelled) {
        setSheets(s);
        setActive(0);
        setLoading(false);
      }
    })().catch((e) => {
      onErr(String(e));
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [relPath, show, onErr]);

  if (!gate.ready || loading) return <div className="preview"><Loading /></div>;
  if (gate.tooBig) return <BigGuard size={gate.size} onForce={gate.force} relPath={relPath} onErr={onErr} />;
  if (!sheets.length) return <div className="preview"><p className="muted">Không có sheet.</p></div>;
  const cur = sheets[active];
  return (
    <div className="preview excel-wrap">
      {sheets.length > 1 && (
        <div className="sheet-tabs">
          <div className="segmented-control" aria-label="Sheet Excel">
            {sheets.map((s, i) => (
              <button
                type="button"
                aria-pressed={active === i}
                className={active === i ? "active" : ""}
                key={s.name}
                onClick={() => setActive(i)}
              >
                {s.name}
              </button>
            ))}
          </div>
        </div>
      )}
      {cur.capped > 0 && (
        <div className="preview-banner-wrap">
          <Notice
            tone="warning"
            action={
              <Button
                variant="ghost"
                size="sm"
                onClick={() => openExternal(relPath, onErr)}
              >
                Mở ngoài
              </Button>
            }
          >
            Sheet lớn ({cur.capped} dòng) — chỉ hiển thị {XLSX_ROW_CAP} dòng đầu.
          </Notice>
        </div>
      )}
      <div className="excel-table" dangerouslySetInnerHTML={{ __html: cur.html }} />
    </div>
  );
}

function BinaryFallback({ node, onErr }: { node: FsNode; onErr: (e: string) => void }) {
  return (
    <div className="preview center-preview">
      <FileQuestion size={40} className="muted" />
      <p>
        Chưa xem trước trực tiếp được <b>.{node.kind}</b> trong app.
      </p>
      <p className="muted">Đối chiếu bản Markdown bên phải, hoặc mở file gốc.</p>
      <Button
        variant="ghost"
        icon={<ExternalLink size={15} />}
        onClick={() => openExternal(node.relPath, onErr)}
      >
        Mở file gốc
      </Button>
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
  switch (cat) {
    case "image":
    case "audio":
      return <BlobMedia relPath={node.relPath} name={node.name} kind={cat} onErr={onError} />;
    case "pdf":
      return <PdfPreview relPath={node.relPath} onErr={onError} />;
    case "docx":
      return <DocxPreview relPath={node.relPath} onErr={onError} />;
    case "excel":
      return <ExcelPreview relPath={node.relPath} onErr={onError} />;
    case "pptx":
      return <PptxPreview relPath={node.relPath} onErr={onError} />;
    case "binary":
      return <BinaryFallback node={node} onErr={onError} />;
    default:
      return <TextPreview relPath={node.relPath} onErr={onError} />;
  }
}
