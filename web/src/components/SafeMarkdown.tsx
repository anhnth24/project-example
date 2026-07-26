import type { ReactNode } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize from 'rehype-sanitize';
import remarkGfm from 'remark-gfm';
import { boundMarkdown, markdownSchema } from '../lib/sanitize';

/**
 * Renders untrusted Markdown (converted document content, grounded Q&A
 * passages) safely: raw HTML is parsed but then sanitized against an
 * allowlist schema (see `lib/sanitize.ts`), and oversized input is clamped
 * before it ever reaches the parser.
 */
export function SafeMarkdown({ children }: { children: string }): ReactNode {
  const { text, truncated } = boundMarkdown(children);

  return (
    <>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeRaw, [rehypeSanitize, markdownSchema]]}
      >
        {text}
      </ReactMarkdown>
      {truncated && (
        <p role="note" className="markdown-truncated">
          Nội dung đã bị rút gọn vì quá lớn để hiển thị an toàn.
        </p>
      )}
    </>
  );
}
