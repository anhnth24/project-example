// Extracted from the Modal component in app/src/components/ui.tsx (desktop),
// where it was an inline effect. No Tauri/filesystem dependency; focuses the
// first focusable element on mount, restores prior focus on unmount, traps
// Tab/Shift+Tab inside the panel, and calls onClose on Escape.
import { useEffect, useRef, type RefObject } from 'react';

export function useFocusTrap(panelRef: RefObject<HTMLElement | null>, onClose: () => void): void {
  const onCloseRef = useRef(onClose);

  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const first =
      panelRef.current?.querySelector<HTMLElement>('[autofocus]') ??
      panelRef.current?.querySelector<HTMLElement>(
        "input, button, textarea, select, [tabindex]:not([tabindex='-1'])",
      );
    first?.focus();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
      } else if (event.key === 'Tab') {
        const focusable = panelRef.current?.querySelectorAll<HTMLElement>(
          "input:not(:disabled), button:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex='-1'])",
        );
        if (!focusable?.length) return;
        const firstElement = focusable[0];
        const lastElement = focusable[focusable.length - 1];
        if (event.shiftKey && document.activeElement === firstElement) {
          event.preventDefault();
          lastElement.focus();
        } else if (!event.shiftKey && document.activeElement === lastElement) {
          event.preventDefault();
          firstElement.focus();
        }
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      previous?.focus();
    };
  }, [panelRef]);
}
