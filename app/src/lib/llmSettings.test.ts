import { describe, expect, it } from "vitest";
import type {
  EmbeddingProviderPreset,
  LlmProviderPreset,
  Settings,
} from "./types";
import {
  applyEmbeddingPreset,
  applyLlmPreset,
  isLocalLlmEndpoint,
  validateEmbeddingSettings,
  validateLlmSettings,
} from "./llmSettings";

const settings: Settings = {
  ocrLangs: "vie+eng",
  pdfOcr: true,
  pdfOcrImages: false,
  audioLang: "vi",
  audioThreads: 4,
  whisperModel: null,
  llmEnabled: false,
  llmProvider: "ollama",
  llmBaseUrl: "http://127.0.0.1:11434",
  llmModel: "qwen2.5:7b",
  llmApiKey: null,
  llmCliBinary: null,
  embeddingEnabled: false,
  embeddingProvider: "ollama",
  embeddingBaseUrl: "http://127.0.0.1:11434",
  embeddingModel: "nomic-embed-text",
  embeddingApiKey: null,
  embeddingDimensions: null,
  embeddingFallbackLocal: true,
};

const ollama: LlmProviderPreset = {
  id: "ollama",
  label: "Ollama",
  provider: "open_ai_compatible",
  baseUrl: "http://127.0.0.1:11434",
  defaultModel: "qwen2.5:7b",
  models: ["qwen2.5:7b"],
  local: true,
  requiresApiKey: false,
  subscription: false,
  supportsVision: true,
  supportsEmbeddings: true,
  description: "Local",
};

const openai: LlmProviderPreset = {
  ...ollama,
  id: "openai",
  label: "OpenAI",
  provider: "open_ai",
  baseUrl: "https://api.openai.com",
  defaultModel: "gpt-4o-mini",
  local: false,
  requiresApiKey: true,
};

const cursor: LlmProviderPreset = {
  ...ollama,
  id: "cursor-cli",
  label: "Cursor subscription",
  provider: "cursor_cli",
  baseUrl: null,
  defaultModel: "auto",
  models: ["auto"],
  local: false,
  subscription: true,
  supportsVision: false,
  supportsEmbeddings: false,
  description: "Official CLI",
};

const embeddingPreset: EmbeddingProviderPreset = {
  id: "openai",
  label: "OpenAI embeddings",
  provider: "open_ai",
  baseUrl: "https://api.openai.com",
  defaultModel: "text-embedding-3-small",
  models: ["text-embedding-3-small"],
  local: false,
  requiresApiKey: true,
  defaultDimensions: 1536,
  description: "Cloud",
};

describe("applyLlmPreset", () => {
  it("enables LLM and applies endpoint/model", () => {
    const result = applyLlmPreset(settings, openai);
    expect(result.llmEnabled).toBe(true);
    expect(result.llmProvider).toBe("openai");
    expect(result.llmBaseUrl).toBe("https://api.openai.com");
    expect(result.llmModel).toBe("gpt-4o-mini");
  });

  it("does not mutate previous settings or erase key", () => {
    const withKey = { ...settings, llmApiKey: "secret" };
    const result = applyLlmPreset(withKey, ollama);
    expect(result).not.toBe(withKey);
    expect(result.llmApiKey).toBe("secret");
  });

  it("clears HTTP credentials for subscription bridge", () => {
    const result = applyLlmPreset(
      { ...settings, llmApiKey: "secret" },
      cursor,
    );
    expect(result.llmProvider).toBe("cursor-cli");
    expect(result.llmBaseUrl).toBe("");
    expect(result.llmApiKey).toBeNull();
  });
});

describe("validateLlmSettings", () => {
  it("accepts disabled LLM", () => {
    expect(validateLlmSettings(settings, openai)).toEqual([]);
  });

  it("accepts local provider without key", () => {
    expect(
      validateLlmSettings({ ...settings, llmEnabled: true }, ollama),
    ).toEqual([]);
  });

  it("requires cloud API key", () => {
    const errors = validateLlmSettings(
      {
        ...settings,
        llmEnabled: true,
        llmProvider: "openai",
        llmBaseUrl: "https://api.openai.com",
        llmModel: "gpt-4o-mini",
      },
      openai,
    );
    expect(errors).toContain("OpenAI yêu cầu API key.");
  });

  it("accepts subscription CLI without URL or API key", () => {
    expect(
      validateLlmSettings(
        {
          ...settings,
          llmEnabled: true,
          llmProvider: "cursor-cli",
          llmBaseUrl: "",
          llmModel: "auto",
        },
        cursor,
      ),
    ).toEqual([]);
  });

  it("rejects missing model and invalid URL", () => {
    const errors = validateLlmSettings(
      {
        ...settings,
        llmEnabled: true,
        llmModel: " ",
        llmBaseUrl: "localhost:11434",
      },
      ollama,
    );
    expect(errors).toHaveLength(2);
  });
});

describe("isLocalLlmEndpoint", () => {
  it("recognizes localhost variants", () => {
    expect(isLocalLlmEndpoint("http://localhost:11434")).toBe(true);
    expect(isLocalLlmEndpoint("http://127.0.0.1:8080")).toBe(true);
    expect(isLocalLlmEndpoint("http://[::1]:8000")).toBe(true);
  });

  it("rejects cloud and invalid endpoints", () => {
    expect(isLocalLlmEndpoint("https://api.openai.com")).toBe(false);
    expect(isLocalLlmEndpoint("not-a-url")).toBe(false);
  });
});

describe("embedding settings", () => {
  it("applies provider model, endpoint and dimensions", () => {
    const result = applyEmbeddingPreset(settings, embeddingPreset);
    expect(result.embeddingEnabled).toBe(true);
    expect(result.embeddingModel).toBe("text-embedding-3-small");
    expect(result.embeddingDimensions).toBe(1536);
  });

  it("validates key, URL and dimension bounds", () => {
    const errors = validateEmbeddingSettings(
      {
        ...settings,
        embeddingEnabled: true,
        embeddingBaseUrl: "bad",
        embeddingModel: "text-embedding-3-small",
        embeddingDimensions: 8,
      },
      embeddingPreset,
    );
    expect(errors).toHaveLength(3);
  });

  it("accepts disabled neural embeddings", () => {
    expect(validateEmbeddingSettings(settings, embeddingPreset)).toEqual([]);
  });
});
