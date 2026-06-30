import {
  FileText,
  FileSpreadsheet,
  FileImage,
  FileAudio,
  FileType2,
  Presentation,
  Globe,
  File,
  type LucideProps,
} from "lucide-react";
import type { FsNode } from "./types";

/** Icon + màu theo loại file (đồng bộ giữa cây và toolbar). */
export function fileIcon(node: FsNode, props: LucideProps = {}) {
  const p = { size: 16, ...props };
  switch (node.kind) {
    case "pdf":
      return <FileType2 {...p} color="#e5484d" />;
    case "docx":
      return <FileText {...p} color="#2f6fed" />;
    case "pptx":
      return <Presentation {...p} color="#e8833a" />;
    case "xlsx":
    case "csv":
      return <FileSpreadsheet {...p} color="#1f9d57" />;
    case "html":
      return <Globe {...p} color="#5b6cff" />;
    case "image":
      return <FileImage {...p} color="#8a63d2" />;
    case "audio":
      return <FileAudio {...p} color="#d6409f" />;
    case "markdown":
      return <FileText {...p} color="#6b7280" />;
    default:
      return <File {...p} color="#9aa0aa" />;
  }
}
