import { createRoot } from 'react-dom/client';
import { App } from './App';
import './styles.css';

// In a mock-mode build (`VITE_MARKHAND_MOCK=1`, used only by the Playwright
// E2E suite — see playwright.config.ts) install the in-browser fetch mock
// before rendering, so React's first request already hits the deterministic
// store. The dynamic import keeps every `mocks/**` module out of the normal
// production bundle: with the flag unset this branch is dead code Rollup drops.
async function bootstrap(): Promise<void> {
  if (import.meta.env.VITE_MARKHAND_MOCK) {
    const { installBrowserMocks } = await import('./mocks/browser');
    installBrowserMocks();
  }
  createRoot(document.getElementById('root')!).render(<App />);
}

void bootstrap();
