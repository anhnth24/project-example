// Shared open/close/position wiring for the rail's two non-navigation
// popovers (org identity, user/account menu). Mirrors the exact pattern
// `components/ui.tsx`'s `SelectControl`/`Combobox` already use — outside
// pointerdown closes, `useFloatingMenu` positions a portalled panel against
// the trigger — plus Escape-to-close-and-refocus-trigger, which those two
// don't need (a listbox's own keydown handler already owns Escape) but a
// plain disclosure popover does.
import { useEffect, useRef, useState, type RefObject } from 'react';
import { useFloatingMenu } from '../../hooks/useFloatingMenu';

export interface RailPopover {
  open: boolean;
  setOpen: (next: boolean | ((current: boolean) => boolean)) => void;
  triggerRef: RefObject<HTMLButtonElement | null>;
  menuRef: RefObject<HTMLDivElement | null>;
  menuStyle: ReturnType<typeof useFloatingMenu>;
}

export function useRailPopover(minWidth = 240): RailPopover {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const menuStyle = useFloatingMenu(open, triggerRef, minWidth);

  useEffect(() => {
    if (!open) return;
    const closeOnOutsideClick = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!triggerRef.current?.contains(target) && !menuRef.current?.contains(target)) {
        setOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false);
        triggerRef.current?.focus();
      }
    };
    document.addEventListener('pointerdown', closeOnOutsideClick);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeOnOutsideClick);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [open]);

  return { open, setOpen, triggerRef, menuRef, menuStyle };
}
