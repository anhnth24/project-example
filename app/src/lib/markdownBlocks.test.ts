import { describe, expect, it } from "vitest";
import {
  alignMarkdownBlocks,
  replaceMarkdownBlock,
  splitMarkdownBlocks,
} from "./markdownBlocks";

describe("splitMarkdownBlocks", () => {
  it("keeps heading sections byte-for-byte", () => {
    const markdown =
      "# Báo cáo\n\nMở đầu.\n\n## Tổng quan\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n## Kết luận\n\nXong.\n";
    const blocks = splitMarkdownBlocks(markdown);

    expect(blocks).toHaveLength(3);
    expect(blocks.map((block) => block.text).join("")).toBe(markdown);
    expect(blocks.map((block) => block.heading)).toEqual([
      "Báo cáo",
      "Tổng quan",
      "Kết luận",
    ]);
  });

  it("falls back to paragraph groups when headings are absent", () => {
    const markdown = "Đoạn một.\n\nĐoạn hai.\n\nĐoạn ba.";
    const blocks = splitMarkdownBlocks(markdown);

    expect(blocks).toHaveLength(3);
    expect(blocks.map((block) => block.text).join("")).toBe(markdown);
  });
});

describe("replaceMarkdownBlock", () => {
  it("changes only the selected range", () => {
    const markdown = "# A\n\nMột.\n\n## B\n\nHai.\n";
    const blocks = splitMarkdownBlocks(markdown);
    const changed = replaceMarkdownBlock(markdown, blocks[1], "## B\n\nĐã sửa.\n");

    expect(changed).toBe("# A\n\nMột.\n\n## B\n\nĐã sửa.\n");
  });
});

describe("alignMarkdownBlocks", () => {
  it("matches repeated headings by sequence and leaves insertions unmatched", () => {
    const source = splitMarkdownBlocks(
      "# Tài liệu\n\n## Ghi chú\n\nMột.\n\n## Ghi chú\n\nHai.\n",
    );
    const draft = splitMarkdownBlocks(
      "# Tài liệu\n\n## Khối mới\n\nMới.\n\n## Ghi chú\n\nMột.\n\n## Ghi chú\n\nHai sửa.\n",
    );
    const aligned = alignMarkdownBlocks(source, draft);

    expect(aligned.map((block) => block?.heading ?? null)).toEqual([
      "Tài liệu",
      null,
      "Ghi chú",
      "Ghi chú",
    ]);
    expect(aligned[3]?.text).toContain("Hai.");
  });
});
