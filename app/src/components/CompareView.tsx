import { useEffect, useMemo, useState } from "react";
import { Check, Link2, Pencil } from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  alignMarkdownBlocks,
  replaceMarkdownBlock,
  splitMarkdownBlocks,
} from "../lib/markdownBlocks";
import type { DocumentSession, FsNode } from "../lib/types";
import { Button } from "./ui";

export function CompareView({
  node,
  session,
  readOnly = false,
  onChange,
}: {
  node: FsNode;
  session: DocumentSession;
  readOnly?: boolean;
  onChange: (markdown: string) => void;
}) {
  const [editing, setEditing] = useState<number | null>(null);
  const [hovered, setHovered] = useState<number | null>(null);
  const sourceBlocks = useMemo(
    () => splitMarkdownBlocks(session.baseline),
    [session.baseline],
  );
  const draftBlocks = useMemo(
    () => splitMarkdownBlocks(session.draft),
    [session.draft],
  );
  const alignedSource = useMemo(
    () => alignMarkdownBlocks(sourceBlocks, draftBlocks),
    [draftBlocks, sourceBlocks],
  );

  useEffect(() => {
    if (readOnly) setEditing(null);
  }, [readOnly]);

  if (!draftBlocks.length) {
    return (
      <div className="compare-empty">
        Chưa có Markdown để đối chiếu. Hãy convert tài liệu trước.
      </div>
    );
  }

  return (
    <div className="compare-view">
      <div className="compare-column-headings">
        <span>Bản convert gốc ({node.kind.toUpperCase()})</span>
        <span aria-hidden="true" />
        <span>Markdown — click để sửa</span>
      </div>
      <div className="compare-scroll">
        {draftBlocks.map((block, index) => {
          const source = alignedSource[index];
          const active = hovered === index || editing === index;
          const changed = source ? source.text !== block.text : true;
          return (
            <article
              className={`compare-pair ${active ? "active" : ""}`}
              key={`${index}-${block.id}`}
              onMouseEnter={() => setHovered(index)}
              onMouseLeave={() => setHovered(null)}
            >
              <section className="source-block" aria-label={`Nguồn của ${block.heading}`}>
                <div className="block-caption">
                  <span>Khối {index + 1}</span>
                  {changed && <b>Đã chỉnh</b>}
                </div>
                <div className="source-markdown markdown-body light-markdown">
                  {source ? (
                    <ReactMarkdown remarkPlugins={[remarkGfm]}>{source.text}</ReactMarkdown>
                  ) : (
                    <p className="unmatched-source">Khối mới — không có trong bản convert gốc.</p>
                  )}
                </div>
              </section>

              <div className="block-connector" aria-hidden="true">
                <span className="connector-line" />
                <span className="connector-node">
                  <Link2 size={10} />
                </span>
              </div>

              <section
                className="markdown-block"
                aria-label={`Markdown của ${block.heading}`}
              >
                {editing === index && !readOnly ? (
                  <>
                    <textarea
                      autoFocus
                      value={block.text}
                      rows={Math.max(5, block.text.split(/\r?\n/).length + 1)}
                      onChange={(event) =>
                        onChange(
                          replaceMarkdownBlock(session.draft, block, event.target.value),
                        )
                      }
                    />
                    <div className="block-edit-footer">
                      <Button
                        variant="primary"
                        size="sm"
                        icon={<Check size={13} />}
                        onClick={() => setEditing(null)}
                      >
                        Xong
                      </Button>
                      <span>Chỉ khối này thay đổi; các khối khác được giữ nguyên.</span>
                    </div>
                  </>
                ) : (
                  <div
                    className="block-preview"
                    role="button"
                    tabIndex={readOnly ? -1 : 0}
                    aria-disabled={readOnly}
                    onClick={() => !readOnly && setEditing(index)}
                    onKeyDown={(event) => {
                      if (!readOnly && (event.key === "Enter" || event.key === " ")) {
                        event.preventDefault();
                        setEditing(index);
                      }
                    }}
                  >
                    <Pencil size={13} className="block-pencil" />
                    <div className="markdown-body">
                      <ReactMarkdown remarkPlugins={[remarkGfm]}>{block.text}</ReactMarkdown>
                    </div>
                  </div>
                )}
              </section>
            </article>
          );
        })}
      </div>
    </div>
  );
}
