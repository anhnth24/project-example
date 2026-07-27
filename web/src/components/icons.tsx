// Organic re-skin: these were hand-authored stand-ins (stroke-width 2, 24x24
// viewBox) because `lucide-react` wasn't declared for web. It is now (see
// web/package.json), so each icon wraps the matching Lucide icon at the
// design system's stroke-width (2.75) instead. Export names are kept as-is
// so `./ui.tsx` (and anything else importing from here) doesn't need to
// change its imports.
import { Check, ChevronDown, LoaderCircle, X } from 'lucide-react';
import type { SVGProps } from 'react';

export type IconProps = SVGProps<SVGSVGElement> & { size?: number };

export function CheckIcon({ size = 15, ...props }: IconProps) {
  return <Check size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function ChevronDownIcon({ size = 15, ...props }: IconProps) {
  return <ChevronDown size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function SpinnerIcon({ size = 15, ...props }: IconProps) {
  return <LoaderCircle size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function CloseIcon({ size = 15, ...props }: IconProps) {
  return <X size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}
