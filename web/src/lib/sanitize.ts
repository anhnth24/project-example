import { defaultSchema } from 'rehype-sanitize';

/**
 * Maximum length (UTF-16 code units) of Markdown source handed to the
 * renderer. `SafeMarkdown` renders converted document content and grounded
 * Q&A passages, both of which are untrusted and can be arbitrarily large (a
 * pathological upload, a scraped page, a run-away extraction). remark/rehype
 * parse cost grows with input size, so an unbounded string lets a single
 * document freeze the tab. 200_000 chars (~200 KB) comfortably covers any
 * legitimate single document preview or answer passage while keeping
 * worst-case parse work bounded.
 */
export const MAX_MARKDOWN_LENGTH = 200_000;

export interface BoundedMarkdown {
  text: string;
  truncated: boolean;
}

/** Clamp Markdown source to `maxLength`, reporting whether it was cut. */
export function boundMarkdown(markdown: string, maxLength = MAX_MARKDOWN_LENGTH): BoundedMarkdown {
  if (markdown.length <= maxLength) {
    return { text: markdown, truncated: false };
  }
  return { text: markdown.slice(0, maxLength), truncated: true };
}

/**
 * rehype-sanitize schema for untrusted Markdown. Starts from the GitHub-style
 * default schema (hast-util-sanitize), which already:
 * - drops `<script>`, `<iframe>`, `<object>`, `<svg>` and any other tag not on
 *   its allowlist (unwrapping unknown tags, fully stripping `script`);
 * - drops every `on*` event-handler attribute and `style` (neither is on the
 *   per-tag attribute allowlist);
 * - restricts `href`/`src`/`cite`/`longDesc` to an allowlisted set of
 *   protocols (`http`, `https`, `mailto`, ...) — this is an allowlist check,
 *   so case games (`JavaScript:`) and whitespace games (`  javascript:`)
 *   don't help an attacker, they just fail to match any allowed protocol;
 * - rewrites clobber-prone `id`/`name`/`aria-describedby`/`aria-labelledby`
 *   with a `user-content-` prefix, defeating DOM-clobbering payloads.
 *
 * We only extend it with `colSpan`/`rowSpan` so converted tables with merged
 * cells keep their shape.
 */
export const markdownSchema = {
  ...defaultSchema,
  attributes: {
    ...defaultSchema.attributes,
    td: [...(defaultSchema.attributes?.td ?? []), 'colSpan', 'rowSpan'],
    th: [...(defaultSchema.attributes?.th ?? []), 'colSpan', 'rowSpan'],
  },
};
