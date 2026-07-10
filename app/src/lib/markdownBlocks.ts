export interface MarkdownBlock {
  id: string;
  heading: string;
  text: string;
  start: number;
  end: number;
}

function titleOf(text: string, index: number): string {
  const heading = text.match(/^#{1,6}[ \t]+(.+)$/m)?.[1]?.trim();
  if (heading) return heading;
  const first = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean);
  return first?.slice(0, 72) || `Khối ${index + 1}`;
}

function blockId(title: string, index: number): string {
  const slug = title
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLocaleLowerCase("vi")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 36);
  return `${index}-${slug || "block"}`;
}

function fromBoundaries(markdown: string, boundaries: number[]): MarkdownBlock[] {
  const unique = [...new Set([0, ...boundaries, markdown.length])]
    .filter((position) => position >= 0 && position <= markdown.length)
    .sort((a, b) => a - b);
  const blocks: MarkdownBlock[] = [];
  for (let i = 0; i < unique.length - 1; i += 1) {
    const start = unique[i];
    const end = unique[i + 1];
    const text = markdown.slice(start, end);
    if (!text.trim()) continue;
    const heading = titleOf(text, blocks.length);
    blocks.push({
      id: blockId(heading, blocks.length),
      heading,
      text,
      start,
      end,
    });
  }
  return blocks;
}

/**
 * Split Markdown without normalising or dropping whitespace. Joining the
 * returned block texts always reproduces the original document, apart from
 * whitespace-only prefixes/suffixes which stay attached to their neighbour.
 */
export function splitMarkdownBlocks(markdown: string): MarkdownBlock[] {
  if (!markdown) return [];

  const headings = [...markdown.matchAll(/^#{1,6}[ \t]+.+$/gm)].map(
    (match) => match.index ?? 0,
  );
  if (headings.length) {
    // Keep introductory content as its own block when the first heading is not
    // at the beginning. Otherwise each heading starts a linked block.
    return fromBoundaries(markdown, headings);
  }

  const paragraphBoundaries = [...markdown.matchAll(/\r?\n[ \t]*\r?\n(?=\S)/g)]
    .map((match) => (match.index ?? 0) + match[0].length);
  return fromBoundaries(markdown, paragraphBoundaries);
}

export function replaceMarkdownBlock(
  markdown: string,
  block: Pick<MarkdownBlock, "start" | "end">,
  replacement: string,
): string {
  return `${markdown.slice(0, block.start)}${replacement}${markdown.slice(block.end)}`;
}

/**
 * Align draft sections to the conversion snapshot using heading-order LCS.
 * Repeated headings are matched by occurrence and inserted sections remain
 * unmatched instead of borrowing an unrelated source block.
 */
export function alignMarkdownBlocks(
  source: MarkdownBlock[],
  draft: MarkdownBlock[],
): Array<MarkdownBlock | null> {
  const key = (block: MarkdownBlock) =>
    block.heading.trim().toLocaleLowerCase("vi");
  const rows = source.length + 1;
  const cols = draft.length + 1;
  const table = Array.from({ length: rows }, () => new Uint32Array(cols));

  for (let i = source.length - 1; i >= 0; i -= 1) {
    for (let j = draft.length - 1; j >= 0; j -= 1) {
      table[i][j] =
        key(source[i]) === key(draft[j])
          ? table[i + 1][j + 1] + 1
          : Math.max(table[i + 1][j], table[i][j + 1]);
    }
  }

  const aligned: Array<MarkdownBlock | null> = Array(draft.length).fill(null);
  let sourceIndex = 0;
  let draftIndex = 0;
  while (sourceIndex < source.length && draftIndex < draft.length) {
    if (key(source[sourceIndex]) === key(draft[draftIndex])) {
      aligned[draftIndex] = source[sourceIndex];
      sourceIndex += 1;
      draftIndex += 1;
    } else if (table[sourceIndex + 1][draftIndex] >= table[sourceIndex][draftIndex + 1]) {
      sourceIndex += 1;
    } else {
      draftIndex += 1;
    }
  }
  return aligned;
}
