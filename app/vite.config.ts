import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Cấu hình Vite cho Tauri: cổng cố định 1420 khớp devUrl trong tauri.conf.json.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Design handoff provides the bundled planet asset one level above app/.
    fs: { allow: [".."] },
  },
  build: {
    target: "es2021",
    outDir: "dist",
  },
});
