import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  build: { target: 'es2021' },
  test: {
    environment: 'jsdom',
    setupFiles: './src/test/setup.ts',
  },
});
