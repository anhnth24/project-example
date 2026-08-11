import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), '');
  return {
    plugins: [react()],
    // assetsInlineLimit 0: never inline assets as `data:` URLs. The served
    // SPA runs under fileconv-server's strict CSP (`img-src 'self'`, see
    // crates/server/src/spa.rs) which rejects data: URIs — Vite's default
    // 4KB inlining silently blanked the brand SVG in real deployments.
    build: { target: 'es2021', assetsInlineLimit: 0 },
    server: {
      proxy: {
        '/api': {
          target: env.MARKHAND_API_ORIGIN || 'http://127.0.0.1:8787',
          changeOrigin: true,
        },
      },
    },
    test: {
      environment: 'jsdom',
      setupFiles: './src/test/setup.ts',
      // Keep vitest to the in-source unit/component tests. The Playwright E2E
      // specs live in `e2e/` and also match `*.spec.ts`; without this bound
      // vitest would try to run them and fail on Playwright's `test()`.
      include: ['src/**/*.{test,spec}.{ts,tsx}'],
    },
  };
});
