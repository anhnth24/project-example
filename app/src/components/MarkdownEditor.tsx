import { useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { markdown } from "@codemirror/lang-markdown";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

type Tab = "edit" | "preview";

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
        <button className={tab === "edit" ? "on" : ""} onClick={() => setTab("edit")}>
          Soạn
        </button>
        <button className={tab === "preview" ? "on" : ""} onClick={() => setTab("preview")}>
          Xem trước
        </button>
      </div>

      {tab === "edit" ? (
        <div className="cm-wrap">
          <CodeMirror
            value={value}
            height="100%"
            extensions={[markdown()]}
            onChange={onChange}
            basicSetup={{ lineNumbers: true, highlightActiveLine: true }}
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
