import { beforeEach, describe, expect, expectTypeOf, it, vi } from "vitest";
import askRequest from "../../src-tauri/fixtures/knowledge/v1/requests/ask.json";
import rebuildRequest from "../../src-tauri/fixtures/knowledge/v1/requests/rebuild.json";
import searchRequest from "../../src-tauri/fixtures/knowledge/v1/requests/search.json";
import askResponse from "../../src-tauri/fixtures/knowledge/v1/responses/ask.json";
import fallbackResponse from "../../src-tauri/fixtures/knowledge/v1/responses/ask-fallback.json";
import rebuildResponse from "../../src-tauri/fixtures/knowledge/v1/responses/rebuild.json";
import searchResponse from "../../src-tauri/fixtures/knowledge/v1/responses/search.json";
import statsResponse from "../../src-tauri/fixtures/knowledge/v1/responses/stats.json";
import { api } from "./ipc";
import type {
  GroundedAnswer,
  HybridAskRequest,
  HybridSearchRequest,
  HybridSearchResponse,
  IndexBuildResult,
  IndexRequest,
  KnowledgeIndexStats,
} from "./types";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

describe("desktop knowledge IPC v1", () => {
  beforeEach(() => invoke.mockReset());

  it("invokes production wrappers with frozen command names and payloads", async () => {
    invoke.mockResolvedValue(undefined);
    await api.rebuildKnowledgeIndex(["payments.pdf", "security.docx"]);
    await api.knowledgeIndexStats();
    await api.hybridSearch(["payments.pdf"], "đối soát", 5);
    await api.hybridAsk(["payments.pdf"], "Khi nào?", 3, true);

    expect(invoke.mock.calls).toEqual([
      [
        "rebuild_knowledge_index",
        { req: { sourceRels: ["payments.pdf", "security.docx"] } },
      ],
      ["knowledge_index_stats"],
      ["hybrid_search", { req: { sourceRels: ["payments.pdf"], query: "đối soát", limit: 5 } }],
      [
        "hybrid_ask",
        {
          req: {
            sourceRels: ["payments.pdf"],
            question: "Khi nào?",
            topK: 3,
            useLlm: true,
          },
        },
      ],
    ]);
  });

  it("freezes request wrappers and camelCase keys", () => {
    expect(rebuildRequest.command).toBe("rebuild_knowledge_index");
    expectTypeOf(rebuildRequest.args.req).toMatchTypeOf<IndexRequest>();
    expect(searchRequest.args.req).toEqual({
      sourceRels: ["payments.pdf", "security.docx"],
      query: "quy trình đối soát thanh toán",
      limit: 20,
    });
    expectTypeOf(searchRequest.args.req).toMatchTypeOf<HybridSearchRequest>();
    expectTypeOf(askRequest.args.req).toMatchTypeOf<HybridAskRequest>();
    expect(askRequest.args.req).toHaveProperty("topK", 8);
    expect(askRequest.args.req).not.toHaveProperty("top_k");
  });

  it("freezes response modes, warnings, anchors and index stats", () => {
    expectTypeOf(rebuildResponse).toMatchTypeOf<IndexBuildResult>();
    expectTypeOf(statsResponse).toMatchTypeOf<KnowledgeIndexStats>();
    expectTypeOf(searchResponse).toMatchTypeOf<HybridSearchResponse>();
    expectTypeOf(askResponse).toMatchTypeOf<Omit<GroundedAnswer, "mode"> & { mode: string }>();
    expectTypeOf(fallbackResponse).toMatchTypeOf<
      Omit<GroundedAnswer, "mode"> & { mode: string }
    >();
    expect(searchResponse.hits[0]?.anchor.page).toBe(7);
    expect(searchResponse.hits[0]?.rerankScore).toBeCloseTo(1.875, 4);
    expect(askResponse.mode).toBe("offline_extractive");
    expect(fallbackResponse.mode).toBe("fallback_extractive");
    const modes: GroundedAnswer["mode"][] = [
      "offline_extractive",
      "local_llm",
      "cloud_llm",
      "subscription_cli",
      "fallback_extractive",
    ];
    expect(modes).toContain(askResponse.mode);
    expect(modes).toContain(fallbackResponse.mode);
    expect(fallbackResponse.grounded).toBe(true);
    expect(fallbackResponse.warnings).toHaveLength(1);
    expect(statsResponse.annThreshold).toBe(1000);
  });
});
