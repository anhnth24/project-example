import CodeMirror from "@uiw/react-codemirror";
import { markdown } from "@codemirror/lang-markdown";
import { EditorView } from "@codemirror/view";
import type { MarkdownTab } from "../lib/types";
import { SafeMarkdown } from "./SafeMarkdown";

const cmFont = {
  fontFamily: "ui-monospace, 'JetBrains Mono', Menlo, Consolas, monospace",
  lineHeight: "1.7",
};

const cmTheme = EditorView.theme({
  "&": {
    fontSize: "13px",
    height: "100%",
    backgroundColor: "#171619",
    color: "#d8d8da",
    fontFamily: "var(--font-ui)",
  },
  ".cm-content": cmFont,
  ".cm-gutters": { backgroundColor: "#1d1c20", border: "none", color: "#68676c" },
  ".cm-activeLine": { backgroundColor: "rgba(255,255,255,0.035)" },
  ".cm-activeLineGutter": {
    backgroundColor: "rgba(46,196,124,0.12)",
    color: "#2ec47c",
  },
  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": {
    backgroundColor: "rgba(46,196,124,0.22)",
  },
  "&.cm-focused": { outline: "2px solid rgba(46,196,124,0.42)", outlineOffset: "-2px" },
});

export function MarkdownEditor({
  value,
  onChange,
  tab,
  onTabChange,
  readOnly = false,
}: {
  value: string;
  onChange: (v: string) => void;
  tab: MarkdownTab;
  onTabChange: (tab: MarkdownTab) => void;
  readOnly?: boolean;
}) {
  return (
    <div className="md-editor">
      <div className="md-tabs">
        <div className="segmented-control" aria-label="Chế độ Markdown">
          <button
            type="button"
            aria-pressed={tab === "edit"}
            className={tab === "edit" ? "active" : ""}
            onClick={() => onTabChange("edit")}
          >
            Soạn
          </button>
          <button
            type="button"
            aria-pressed={tab === "preview"}
            className={tab === "preview" ? "active" : ""}
            onClick={() => onTabChange("preview")}
          >
            Xem trước
          </button>
        </div>
      </div>

      {tab === "edit" ? (
        <div className="cm-wrap">
          <CodeMirror
            value={value}
            height="100%"
            extensions={[
              markdown(),
              EditorView.lineWrapping,
              cmTheme,
              EditorView.editable.of(!readOnly),
            ]}
            onChange={onChange}
            basicSetup={{ lineNumbers: true, highlightActiveLine: true, foldGutter: false }}
          />
        </div>
      ) : (
        <div className="md-preview markdown-body">
          <SafeMarkdown>{value}</SafeMarkdown>
        </div>
      )}
    </div>
  );
}
