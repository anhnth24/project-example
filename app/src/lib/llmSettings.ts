import type { LlmProviderPreset, Settings } from "./types";

export function applyLlmPreset(
  settings: Settings,
  preset: LlmProviderPreset,
): Settings {
  return {
    ...settings,
    llmEnabled: true,
    llmProvider: preset.id,
    llmBaseUrl: preset.baseUrl ?? settings.llmBaseUrl,
    llmModel: preset.defaultModel,
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
