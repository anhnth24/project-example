import { useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { markdown } from "@codemirror/lang-markdown";
import { EditorView } from "@codemirror/view";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { TabList, Tab } from "@astryxdesign/core/TabList";

type TabVal = "edit" | "preview";

const cmTheme = EditorView.theme({
  "&": {
    fontSize: "13px",
    height: "100%",
    backgroundColor: "#ffffff",
    fontFamily: "var(--font-ui)",
  },
  ".cm-content": { fontFamily: "ui-monospace, 'JetBrains Mono', Menlo, Consolas, monospace" },
  ".cm-gutters": { backgroundColor: "#f8fafc", border: "none", color: "#94a3b8" },
  ".cm-activeLine": { backgroundColor: "#eff6ff" },
  ".cm-activeLineGutter": { backgroundColor: "#dbeafe", color: "#1d4ed8" },
  "&.cm-focused": { outline: "none" },
});

export function MarkdownEditor({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const [tab, setTab] = useState<TabVal>("edit");

  return (
    <div className="md-editor">
      <div className="md-tabs">
        <TabList value={tab} onChange={(v: string) => setTab(v as TabVal)} size="sm">
          <Tab value="edit" label="Soạn" />
          <Tab value="preview" label="Xem trước" />
        </TabList>
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
