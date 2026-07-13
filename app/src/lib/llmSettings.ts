import type {
  EmbeddingProviderPreset,
  LlmProviderPreset,
  Settings,
} from "./types";

export function applyLlmPreset(
  settings: Settings,
  preset: LlmProviderPreset,
): Settings {
  return {
    ...settings,
    llmEnabled: true,
    llmProvider: preset.id,
    llmBaseUrl: preset.subscription ? "" : (preset.baseUrl ?? settings.llmBaseUrl),
    llmModel: preset.defaultModel,
    llmApiKey: preset.subscription ? null : settings.llmApiKey,
  };
}

export function validateLlmSettings(
  settings: Settings,
  preset?: LlmProviderPreset,
): string[] {
  if (!settings.llmEnabled) return [];
  const errors: string[] = [];
  if (!settings.llmModel.trim()) errors.push("Model LLM không được để trống.");
  if (
    !preset?.subscription &&
    !settings.llmBaseUrl.startsWith("http://") &&
    !settings.llmBaseUrl.startsWith("https://")
  ) {
    errors.push("LLM base URL phải bắt đầu bằng http:// hoặc https://.");
  }
  if (preset?.requiresApiKey && !settings.llmApiKey?.trim()) {
    errors.push(`${preset.label} yêu cầu API key.`);
  }
  return errors;
}

export function isLocalLlmEndpoint(baseUrl: string): boolean {
  try {
    const url = new URL(baseUrl);
    return ["localhost", "127.0.0.1", "::1", "[::1]"].includes(url.hostname);
  } catch {
    return false;
  }
}

export function applyEmbeddingPreset(
  settings: Settings,
  preset: EmbeddingProviderPreset,
): Settings {
  return {
    ...settings,
    embeddingEnabled: true,
    embeddingProvider: preset.id,
    embeddingBaseUrl: preset.baseUrl ?? settings.embeddingBaseUrl,
    embeddingModel: preset.defaultModel,
    embeddingDimensions: preset.defaultDimensions,
  };
}

export function validateEmbeddingSettings(
  settings: Settings,
  preset?: EmbeddingProviderPreset,
): string[] {
  if (!settings.embeddingEnabled) return [];
  const errors: string[] = [];
  if (!settings.embeddingModel.trim()) {
    errors.push("Model embedding không được để trống.");
  }
  if (
    !settings.embeddingBaseUrl.startsWith("http://") &&
    !settings.embeddingBaseUrl.startsWith("https://")
  ) {
    errors.push("Embedding base URL phải bắt đầu bằng http:// hoặc https://.");
  }
  if (preset?.requiresApiKey && !settings.embeddingApiKey?.trim()) {
    errors.push(`${preset.label} yêu cầu API key.`);
  }
  const dimensions = settings.embeddingDimensions;
  if (dimensions != null && (!Number.isInteger(dimensions) || dimensions < 32 || dimensions > 4096)) {
    errors.push("Số chiều embedding phải nằm trong khoảng 32–4096.");
  }
  return errors;
}
