import { describe, expect, it } from "vitest";
import type { FsNode } from "./types";
import {
  filesInProject,
  findByRel,
  flattenFiles,
  nodeMatches,
  parentRel,
} from "./tree";

const tree: FsNode = {
  name: "DATA",
  relPath: "",
  isDir: true,
  kind: "folder",
  supported: false,
  mdRelPath: null,
  standaloneMd: false,
  children: [
    {
      name: "Nghiệp vụ",
      relPath: "nghiep-vu",
      isDir: true,
      kind: "folder",
      supported: false,
      mdRelPath: null,
      standaloneMd: false,
      children: [
        {
          name: "bao-cao.pdf",
          relPath: "nghiep-vu/bao-cao.pdf",
          isDir: false,
          kind: "pdf",
          supported: true,
          mdRelPath: "nghiep-vu/bao-cao.pdf.md",
          standaloneMd: false,
          children: [],
        },
      ],
    },
  ],
};

describe("tree helpers", () => {
  it("finds and flattens nested files", () => {
    expect(findByRel(tree, "nghiep-vu/bao-cao.pdf")?.name).toBe("bao-cao.pdf");
    expect(flattenFiles(tree).map((node) => node.relPath)).toEqual([
      "nghiep-vu/bao-cao.pdf",
    ]);
  });

  it("keeps a parent visible when a child matches search", () => {
    expect(nodeMatches(tree.children[0], "báo-cáo")).toBe(true);
    expect(nodeMatches(tree.children[0], "bao-cao")).toBe(true);
  });

  it("derives a relative parent", () => {
    expect(parentRel("nghiep-vu/bao-cao.pdf")).toBe("nghiep-vu");
    expect(parentRel("bao-cao.pdf")).toBe("");
  });

  it("scopes files to a selected project root", () => {
    expect(
      filesInProject(tree, {
        id: "project-1",
        name: "Nghiệp vụ",
        rootRel: "nghiep-vu",
        createdAt: 0,
        importedFrom: null,
        implicit: false,
      }).map((node) => node.relPath),
    ).toEqual(["nghiep-vu/bao-cao.pdf"]);
  });

  it("legacy root project sees all files", () => {
    expect(
      filesInProject(tree, {
        id: "legacy-root",
        name: "DATA",
        rootRel: "",
        createdAt: 0,
        importedFrom: null,
        implicit: true,
      }),
    ).toHaveLength(1);
  });
});
