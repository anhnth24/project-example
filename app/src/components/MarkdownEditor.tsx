import { useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { markdown } from "@codemirror/lang-markdown";
import { EditorView } from "@codemirror/view";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Pencil, Eye } from "lucide-react";

type Tab = "edit" | "preview";

const cmTheme = EditorView.theme({
  "&": { fontSize: "13px", height: "100%", backgroundColor: "#ffffff" },
  ".cm-gutters": { backgroundColor: "#fbfbfd", border: "none", color: "#b9bcc4" },
  ".cm-activeLine": { backgroundColor: "#f7f8ff" },
  ".cm-activeLineGutter": { backgroundColor: "#f0f1fb" },
  "&.cm-focused": { outline: "none" },
});

export function MarkdownEditor({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const [tab, setTab] = useState<Tab>("edit");

  return (
    <div className="md-editor">
      <div className="md-tabs">
        <button className={`seg ${tab === "edit" ? "on" : ""}`} onClick={() => setTab("edit")}>
          <Pencil size={14} /> Soạn
        </button>
        <button className={`seg ${tab === "preview" ? "on" : ""}`} onClick={() => setTab("preview")}>
          <Eye size={14} /> Xem trước
        </button>
      </div>

      {tab === "edit" ? (
        <div className="cm-wrap">
          <CodeMirror
            value={value}
            height="100%"
            extensions={[markdown(), EditorView.lineWrapping, cmTheme]}
            onChange={onChange}
            basicSetup={{ lineNumbers: true, highlightActiveLine: true, foldGutter: false }}
          />
        </div>
      ) : (
        <div className="md-preview markdown-body">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{value}</ReactMarkdown>
        </div>
      )}
    </div>
  );
}
