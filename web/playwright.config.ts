import { existsSync } from 'node:fs';
import { defineConfig, devices } from '@playwright/test';

// The P2-15 E2E suite drives a real browser against a *mock-mode* dev server
// (`VITE_MARKHAND_MOCK=1`), so the whole flow is deterministic and needs no
// live backend. The real-deployment half of P2-15 (against Postgres/Qdrant/
// MinIO) is out of this config's scope — it belongs to the `dev-stack` job and
// is deferred until a stack is available. `ask → citation` is likewise absent:
// the Q&A page is a placeholder until P2-10 ships (blocked on R02/R03/R05).

// This environment ships a pre-installed Chromium at a fixed path and blocks
// re-downloads; CI (GitHub ubuntu) has none, so it runs `playwright install
// chromium` first and Playwright resolves the browser itself. Use the fixed
// path only when it exists, so the same config works in both places.
const PREINSTALLED_CHROMIUM = '/opt/pw-browsers/chromium';
const executablePath = existsSync(PREINSTALLED_CHROMIUM) ? PREINSTALLED_CHROMIUM : undefined;

const PORT = 4173;

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [['github'], ['list']] : [['list']],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: 'on-first-retry',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'], launchOptions: { executablePath } },
    },
  ],
  webServer: {
    // Vite dev is enough for E2E and avoids a separate build step; the mock
    // flag is read at runtime via `import.meta.env`.
    command: `VITE_MARKHAND_MOCK=1 pnpm exec vite --port ${PORT} --strictPort`,
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
