import { useEffect, useRef, useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Loader2,
  Presentation,
} from "lucide-react";
import { api } from "../lib/ipc";
import type {
  PptxPreviewMeta,
  PptxPreviewShape,
  PptxPreviewSlide,
} from "../lib/types";
import { Button, Notice } from "./ui";

const PPTX_LIMIT = 40 * 1024 * 1024;
const EMU_PER_POINT = 12_700;

function renderShape(shape: PptxPreviewShape, index: number) {
  if (shape.kind === "image") {
    return (
      <image
        key={index}
        x={shape.x}
        y={shape.y}
        width={shape.width}
        height={shape.height}
        href={shape.dataUrl}
        preserveAspectRatio="xMidYMid meet"
        aria-label={shape.alt || "Ảnh trong slide"}
      />
    );
  }
  if (shape.kind === "shape") {
    return (
      <rect
        key={index}
        x={shape.x}
        y={shape.y}
        width={shape.width}
        height={shape.height}
        fill={shape.fill ?? "transparent"}
        stroke={shape.stroke ?? "transparent"}
        strokeWidth={12_000}
      />
    );
  }
  return (
    <g key={index}>
      {shape.fill && (
        <rect
          x={shape.x}
          y={shape.y}
          width={shape.width}
          height={shape.height}
          fill={shape.fill}
        />
      )}
      <foreignObject
        x={shape.x}
        y={shape.y}
        width={shape.width}
        height={shape.height}
      >
        <div
          className="pptx-text-shape"
          style={{
            color: shape.color,
            fontSize: `${shape.fontPt * EMU_PER_POINT}px`,
            fontWeight: shape.bold ? 700 : 400,
          }}
        >
          {shape.text}
        </div>
      </foreignObject>
    </g>
  );
}

export function PptxPreview({
  relPath,
  onErr,
}: {
  relPath: string;
  onErr: (error: string) => void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const cacheRef = useRef(new Map<number, PptxPreviewSlide>());
  const [meta, setMeta] = useState<PptxPreviewMeta | null>(null);
  const [slide, setSlide] = useState<PptxPreviewSlide | null>(null);
  const [active, setActive] = useState(0);
  const [size, setSize] = useState<number | null>(null);
  const [forced, setForced] = useState(false);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    cacheRef.current.clear();
    setMeta(null);
    setSlide(null);
    setActive(0);
    setForced(false);
    setLoading(true);
    api
      .fileSize(relPath)
      .then((bytes) => {
        if (!cancelled) setSize(bytes);
      })
      .catch((error) => {
        if (!cancelled) onErr(String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [relPath, onErr]);

  const canRender = size != null && (size <= PPTX_LIMIT || forced);

  useEffect(() => {
    if (!canRender) return;
    let cancelled = false;
    setLoading(true);
    api
      .previewPptxMeta(relPath)
      .then((value) => {
        if (!cancelled) setMeta(value);
      })
      .catch((error) => {
        if (!cancelled) {
          setLoading(false);
          onErr(String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [canRender, relPath, onErr]);

  useEffect(() => {
    if (!meta?.slideCount) {
      if (meta) setLoading(false);
      return;
    }
    const cached = cacheRef.current.get(active);
    if (cached) {
      setSlide(cached);
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    api
      .previewPptxSlide(relPath, active)
      .then((value) => {
        if (cancelled) return;
        cacheRef.current.set(active, value);
        setSlide(value);
        setLoading(false);
      })
      .catch((error) => {
        if (!cancelled) {
          setLoading(false);
          onErr(String(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [active, meta, relPath, onErr]);

  function goTo(index: number) {
    if (!meta) return;
    setActive(Math.max(0, Math.min(meta.slideCount - 1, index)));
    rootRef.current?.focus();
  }

  async function openExternal() {
    try {
      await openPath(await api.resolvePath(relPath));
    } catch (error) {
      onErr(String(error));
    }
  }

  if (size == null) {
    return (
      <div className="preview pptx-preview">
        <Loader2 className="spin" size={22} />
      </div>
    );
  }
  if (size > PPTX_LIMIT && !forced) {
    return (
      <div className="preview center-preview">
        <Presentation size={40} className="muted" />
        <p>Presentation lớn có thể dùng nhiều bộ nhớ khi giải nén ảnh.</p>
        <div className="guard-actions">
          <Button variant="ghost" onClick={() => setForced(true)}>
            Vẫn xem trong app
          </Button>
          <Button
            variant="primary"
            icon={<ExternalLink size={15} />}
            onClick={openExternal}
          >
            Mở ngoài
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div
      ref={rootRef}
      className="preview pptx-preview"
      role="region"
      aria-label="Xem trước PowerPoint"
      tabIndex={0}
      onKeyDown={(event) => {
        if (event.key === "ArrowLeft") goTo(active - 1);
        else if (event.key === "ArrowRight") goTo(active + 1);
        else if (event.key === "Home") goTo(0);
        else if (event.key === "End" && meta) goTo(meta.slideCount - 1);
      }}
    >
      <div className="pptx-toolbar">
        <Button
          variant="ghost"
          size="sm"
          icon={<ChevronLeft size={15} />}
          disabled={active <= 0}
          aria-label="Slide trước"
          onClick={() => goTo(active - 1)}
        >
          Trước
        </Button>
        <span aria-live="polite">
          Slide {meta?.slideCount ? active + 1 : 0} / {meta?.slideCount ?? 0}
        </span>
        <Button
          variant="ghost"
          size="sm"
          icon={<ChevronRight size={15} />}
          disabled={!meta || active >= meta.slideCount - 1}
          aria-label="Slide sau"
          onClick={() => goTo(active + 1)}
        >
          Sau
        </Button>
        <span className="pptx-toolbar-spacer" />
        <Button
          variant="ghost"
          size="sm"
          icon={<ExternalLink size={14} />}
          onClick={openExternal}
        >
          Mở ngoài
        </Button>
      </div>

      {!loading && meta?.slideCount === 0 && (
        <Notice tone="warning">Presentation không có slide.</Notice>
      )}
      <div className="pptx-stage-wrap">
        {slide && (
          <svg
            className="pptx-stage"
            viewBox={`0 0 ${slide.widthEmu} ${slide.heightEmu}`}
            role="img"
            aria-label={`Slide ${active + 1}`}
            style={{ background: slide.background }}
          >
            {slide.shapes.map(renderShape)}
          </svg>
        )}
        {loading && (
          <div className="preview-loading">
            <Loader2 className="spin" size={22} />
            <span>Đang dựng slide…</span>
          </div>
        )}
      </div>
      <small className="pptx-fidelity-note">
        Preview hỗ trợ text, ảnh và shape cơ bản; chart/SmartArt phức tạp có thể
        cần mở bằng PowerPoint hoặc LibreOffice.
      </small>
    </div>
  );
}
