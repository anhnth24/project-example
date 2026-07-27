import { Moon, Sun } from 'lucide-react';
import { RailHint } from './RailHint';
import { useTheme } from './useTheme';

export function ThemeToggle() {
  const { theme, toggleTheme } = useTheme();
  const isDark = theme === 'dark';
  const label = isDark ? 'Chuyển sang giao diện sáng' : 'Chuyển sang giao diện tối';

  return (
    <RailHint label={label}>
      <button
        type="button"
        className="btn btn-icon rail-btn"
        aria-label={label}
        aria-pressed={isDark}
        onClick={toggleTheme}
      >
        {isDark ? (
          <Moon size={19} strokeWidth={2.75} aria-hidden="true" />
        ) : (
          <Sun size={19} strokeWidth={2.75} aria-hidden="true" />
        )}
      </button>
    </RailHint>
  );
}
