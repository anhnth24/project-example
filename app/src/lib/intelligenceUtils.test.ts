import { describe, expect, it } from "vitest";
import {
  canGenerateHandoff,
  intelligenceSlug,
  reconcileIntelligenceScope,
  sameScope,
  toggleScopeItem,
  updateTableCell,
} from "./intelligenceUtils";

describe("intelligenceSlug", () => {
  it("folds Vietnamese accents", () => {
    expect(intelligenceSlug("Đối soát Giao dịch")).toBe("doi-soat-giao-dich");
  });

  it("collapses punctuation and trims separators", () => {
    expect(intelligenceSlug("  BRD / PRD --- Alpha  ")).toBe("brd-prd-alpha");
  });

  it("limits path length", () => {
    expect(intelligenceSlug("a".repeat(100))).toHaveLength(60);
  });
});

describe("reconcileIntelligenceScope", () => {
  it("keeps valid selected documents", () => {
    expect(reconcileIntelligenceScope(["b"], ["a", "b"])).toEqual(["b"]);
  });

  it("removes deleted documents", () => {
    expect(reconcileIntelligenceScope(["a", "missing"], ["a", "b"])).toEqual(["a"]);
  });

  it("defaults to all documents when prior scope is empty", () => {
    expect(reconcileIntelligenceScope([], ["a", "b"])).toEqual(["a", "b"]);
  });

  it("remains empty when corpus is empty", () => {
    expect(reconcileIntelligenceScope(["old"], [])).toEqual([]);
  });
});

describe("toggleScopeItem", () => {
  it("adds a missing item", () => {
    expect(toggleScopeItem(["a"], "b")).toEqual(["a", "b"]);
  });

  it("removes an existing item without mutating input", () => {
    const input = ["a", "b"];
    expect(toggleScopeItem(input, "a")).toEqual(["b"]);
    expect(input).toEqual(["a", "b"]);
  });
});

describe("updateTableCell", () => {
  it("updates only the targeted cell", () => {
    expect(
      updateTableCell(
        [
          ["A", "B"],
          ["1", "2"],
        ],
        1,
        0,
        "10",
      ),
    ).toEqual([
      ["A", "B"],
      ["10", "2"],
    ]);
  });

  it("does not mutate original rows", () => {
    const rows = [["A"], ["1"]];
    updateTableCell(rows, 1, 0, "2");
    expect(rows).toEqual([["A"], ["1"]]);
  });

  it("ignores an out-of-range coordinate", () => {
    expect(updateTableCell([["A"]], 5, 5, "x")).toEqual([["A"]]);
  });
});

describe("scope and generation guards", () => {
  it("compares scopes without depending on order", () => {
    expect(sameScope(["a", "b"], ["b", "a"])).toBe(true);
    expect(sameScope(["a"], ["a", "b"])).toBe(false);
  });

  it("requires both corpus and product name", () => {
    expect(canGenerateHandoff(["a"], "Markhand")).toBe(true);
    expect(canGenerateHandoff([], "Markhand")).toBe(false);
    expect(canGenerateHandoff(["a"], "   ")).toBe(false);
  });
});
