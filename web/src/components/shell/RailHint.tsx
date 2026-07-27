// Every control in the rail is icon-only, so each one needs both an
// accessible name (the child supplies `aria-label`/`aria-labelledby` itself —
// this wrapper does not) *and* a visible label a sighted mouse or keyboard
// user can see without a screen reader. A native `title` attribute alone
// does not reliably surface on keyboard focus across browsers, so this
// renders a real tooltip element shown by CSS on `:hover`/`:focus-within` of
// the wrapper (see `.rail-item`/`.rail-tooltip` in styles.css) — it works for
// both a mouse hovering the control and a keyboard user tabbing to it.
import type { ReactNode } from 'react';

export function RailHint({
  label,
  children,
  side = 'right',
}: {
  label: string;
  children: ReactNode;
  /** Which edge of the control the tooltip opens toward. The rail sits at
   * the left viewport edge, so its own items open to the right; a menu that
   * itself renders inside a popover anchored off the rail (none currently
   * do) could open the other way. */
  side?: 'right' | 'top';
}) {
  return (
    <span className={`rail-item rail-item-${side}`}>
      {children}
      <span className="rail-tooltip" role="tooltip">
        {label}
      </span>
    </span>
  );
}
