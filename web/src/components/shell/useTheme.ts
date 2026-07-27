// Minimal light/dark theme seam for the rail's theme toggle.
//
// There is no theme system anywhere else in this codebase: organic-styles.css
// (the design-system source of truth) and web/src/styles.css both define a
// single, light-only token set — no `prefers-color-scheme` handling, no
// `data-theme` hook, no dark ramp. Per the shell brief, a theme toggle must
// "actually do something or not be rendered at all"; this file is the
// minimal real thing rather than a dead button: it stamps `data-theme` on
// `<html>` and styles.css (appended, not rewritten) supplies a
// `:root[data-theme='dark']` override for the same token names every
// component already reads. Toggling it repaints the whole app for real.
//
// Persisted to localStorage so a reload keeps the visitor's choice; falls
// back to the OS `prefers-color-scheme` on first visit.
import { useCallback, useEffect, useState } from 'react';

export type ThemeName = 'light' | 'dark';

const STORAGE_KEY = 'markhand:theme';

function prefersDark(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: dark)').matches
  );
}

function readStoredTheme(): ThemeName | null {
  if (typeof window === 'undefined') return null;
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    return stored === 'light' || stored === 'dark' ? stored : null;
  } catch {
    // Storage can throw (private mode, quota) — falling back to system
    // preference is a safe, silent degrade, not a broken toggle.
    return null;
  }
}

function initialTheme(): ThemeName {
  return readStoredTheme() ?? (prefersDark() ? 'dark' : 'light');
}

function applyTheme(theme: ThemeName): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.theme = theme;
}

export function useTheme(): { theme: ThemeName; toggleTheme: () => void } {
  const [theme, setTheme] = useState<ThemeName>(initialTheme);

  useEffect(() => {
    applyTheme(theme);
    try {
      window.localStorage.setItem(STORAGE_KEY, theme);
    } catch {
      // Best-effort persistence only.
    }
  }, [theme]);

  const toggleTheme = useCallback(() => {
    setTheme((current) => (current === 'dark' ? 'light' : 'dark'));
  }, []);

  return { theme, toggleTheme };
}
