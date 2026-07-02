// Theme sáng/tối. Tối là mặc định (app tiện ích — đỡ chói khi làm việc lâu),
// lưu lựa chọn vào localStorage, áp qua <html data-theme="...">.
import { useSyncExternalStore } from "react";

export type Theme = "light" | "dark";
const KEY = "markhand-theme";

let listeners: Array<() => void> = [];

function current(): Theme {
  return (localStorage.getItem(KEY) as Theme) || "dark";
}

export function applyTheme(t: Theme) {
  document.documentElement.dataset.appearance = t;
  localStorage.setItem(KEY, t);
  listeners.forEach((l) => l());
}

export function initTheme() {
  document.documentElement.dataset.appearance = current();
}

export function useTheme(): [Theme, () => void] {
  const theme = useSyncExternalStore(
    (cb) => {
      listeners.push(cb);
      return () => {
        listeners = listeners.filter((l) => l !== cb);
      };
    },
    current,
  );
  return [theme, () => applyTheme(theme === "dark" ? "light" : "dark")];
}
