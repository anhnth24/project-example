// Extracted from app/src/components/ui.tsx (desktop), where it was a private
// helper inside the same file as SelectControl/Combobox. No Tauri/filesystem
// dependency; positions a floating menu (a portalled listbox) against an
// anchor element, flipping above the anchor when there is not enough room
// below.
import { useLayoutEffect, useState, type CSSProperties, type RefObject } from 'react';

export function useFloatingMenu(
  open: boolean,
  anchorRef: RefObject<HTMLElement | null>,
  minWidth = 200,
): CSSProperties | null {
  const [style, setStyle] = useState<CSSProperties | null>(null);

  useLayoutEffect(() => {
    // No setState here for the closed case: callers already gate rendering
    // on `open` (see the `return open ? style : null` below), so a stale
    // `style` value sitting unused in state while closed is harmless and
    // avoids a bare `setState` call at the top of the effect body.
    if (!open) return;
    const update = () => {
      const anchor = anchorRef.current;
      if (!anchor) return;
      const rect = anchor.getBoundingClientRect();
      const menuWidth = Math.min(Math.max(rect.width, minWidth), window.innerWidth - 16);
      const spaceBelow = window.innerHeight - rect.bottom - 12;
      const spaceAbove = rect.top - 12;
      const opensAbove = spaceBelow < 160 && spaceAbove > spaceBelow;
      const available = opensAbove ? spaceAbove : spaceBelow;
      const left = Math.min(Math.max(8, rect.left), window.innerWidth - menuWidth - 8);
      setStyle({
        left,
        width: menuWidth,
        maxHeight: Math.min(280, Math.max(96, available)),
        ...(opensAbove ? { bottom: window.innerHeight - rect.top + 6 } : { top: rect.bottom + 6 }),
      });
    };
    update();
    window.addEventListener('resize', update);
    window.addEventListener('scroll', update, true);
    return () => {
      window.removeEventListener('resize', update);
      window.removeEventListener('scroll', update, true);
    };
  }, [anchorRef, minWidth, open]);

  return open ? style : null;
}
