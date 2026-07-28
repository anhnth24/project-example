/// <reference types="vite/client" />

// Augments vite/client's `ImportMetaEnv` with the two app-specific vars.
interface ImportMetaEnv {
  /**
   * Truthy (`"1"`) only for the mock-mode build the Playwright E2E suite
   * drives (playwright.config.ts). When set, `main.tsx` installs the in-browser
   * fetch mock before rendering. Unset in every real build.
   */
  readonly VITE_MARKHAND_MOCK?: string;
  /** Overrides the API base URL; see `api/client.ts`. */
  readonly VITE_MARKHAND_API_BASE_URL?: string;
}
