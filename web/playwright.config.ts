import { existsSync } from 'node:fs';
import { defineConfig, devices, type PlaywrightTestConfig } from '@playwright/test';

// The P2-15 E2E suite drives a real browser against a *mock-mode* dev server
// (`VITE_MARKHAND_MOCK=1`), so the whole flow is deterministic and needs no
// live backend. `ask → citation` (P2-10, `e2e/qa.spec.ts`) is covered here
// too now — the owner lowered P2-10's gate 2026-07-29 to build on the
// contract + mock server rather than waiting on R02/R03/R05.
//
// The real-deployment half of P2-15 lives in `e2e-real/` and runs as the
// `real` project below, driven only by `deploy/scripts/web-e2e-real.sh`
// against an already-up dev stack (Postgres/Qdrant/MinIO/embedding + a
// built `fileconv-server` serving `web/dist` — see that script and
// `deploy/README.md`'s "Web SPA static serving" section). `MARKHAND_E2E_REAL`
// gates which half of this config exists at all: the two projects/webServer
// setups are mutually exclusive, not just filtered by `--project`, so a
// plain `pnpm --dir web e2e` (CI's `web-e2e` job, local dev) never has a
// `real` project to accidentally pick up, and `web-e2e-real.sh` never starts
// the mock vite dev server it doesn't need.

// This environment ships a pre-installed Chromium at a fixed path and blocks
// re-downloads; CI (GitHub ubuntu) has none, so it runs `playwright install
// chromium` first and Playwright resolves the browser itself. Use the fixed
// path only when it exists, so the same config works in both places.
const PREINSTALLED_CHROMIUM = '/opt/pw-browsers/chromium';

type EnvLike = Record<string, string | undefined>;

export function createPlaywrightConfig(
  env: EnvLike = process.env as EnvLike,
): PlaywrightTestConfig {
  const REAL_MODE = env.MARKHAND_E2E_REAL === '1';
  const executablePath = existsSync(PREINSTALLED_CHROMIUM) ? PREINSTALLED_CHROMIUM : undefined;

  const PORT = 4173;
  // fileconv-server's dev bind addr (deploy/dev/.env.example); web-e2e-real.sh
  // exports this when MARKHAND_BIND_ADDR differs from the default.
  const REAL_BASE_URL = env.MARKHAND_E2E_REAL_BASE_URL ?? 'http://127.0.0.1:8787';

  return defineConfig({
    testDir: './e2e',
    fullyParallel: !REAL_MODE,
    workers: REAL_MODE ? 1 : undefined,
    forbidOnly: !!env.CI,
    retries: env.CI ? 1 : 0,
    reporter: env.CI ? [['github'], ['list']] : [['list']],
    use: {
      baseURL: REAL_MODE ? REAL_BASE_URL : `http://127.0.0.1:${PORT}`,
      trace: REAL_MODE ? 'off' : 'on-first-retry',
      ...(REAL_MODE ? { screenshot: 'off', video: 'off' } : {}),
    },
    projects: REAL_MODE
      ? [
          {
            name: 'real',
            testDir: './e2e-real',
            use: { ...devices['Desktop Chrome'], launchOptions: { executablePath } },
          },
        ]
      : [
          {
            name: 'chromium',
            use: { ...devices['Desktop Chrome'], launchOptions: { executablePath } },
          },
        ],
    // No webServer in real mode: web-e2e-real.sh starts fileconv-server itself
    // (serving both the API and the built `web/dist` SPA) before Playwright
    // runs, and stops it afterwards — Playwright only needs to poll `baseURL`.
    webServer: REAL_MODE
      ? undefined
      : {
          // Bind the dev server explicitly to 127.0.0.1 so it matches the IPv4
          // `url` Playwright polls: with a bare `localhost` bind, a CI runner
          // that resolves `localhost` to `::1` first leaves the IPv4 health
          // check hanging until the timeout (observed on GitHub Actions). The
          // mock flag is read at runtime via `import.meta.env`, so `vite` dev
          // needs no separate build step.
          command: `pnpm exec vite --host 127.0.0.1 --port ${PORT} --strictPort`,
          url: `http://127.0.0.1:${PORT}`,
          reuseExistingServer: !env.CI,
          timeout: 120_000,
          stdout: 'pipe',
          stderr: 'pipe',
          env: {
            ...(env as Record<string, string | undefined>),
            VITE_MARKHAND_MOCK: '1',
          },
        },
  });
}

export default createPlaywrightConfig();
