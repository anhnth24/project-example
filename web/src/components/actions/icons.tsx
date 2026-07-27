// Same wrapping convention as `components/icons.tsx` (stroke-width 2.75,
// aria-hidden) applied to the extra glyphs this component needs that the
// shared file doesn't export. Kept local to `components/actions/**` rather
// than adding to `components/icons.tsx`, which this task must not touch.
import { Download, RefreshCw, RotateCcw, Trash2 } from 'lucide-react';
import type { SVGProps } from 'react';

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

export function DownloadIcon({ size = 15, ...props }: IconProps) {
  return <Download size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function ReindexIcon({ size = 15, ...props }: IconProps) {
  return <RefreshCw size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function RetryIcon({ size = 15, ...props }: IconProps) {
  return <RotateCcw size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function DeleteIcon({ size = 15, ...props }: IconProps) {
  return <Trash2 size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}
