import { describe, expect, it } from "vitest";
import type { LlmProviderPreset, Settings } from "./types";
import {
  applyLlmPreset,
  isLocalLlmEndpoint,
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
