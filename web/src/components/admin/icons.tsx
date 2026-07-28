// Same wrapping convention as `components/icons.tsx`/`components/actions/icons.tsx`
// (stroke-width 2.75, aria-hidden) for the glyphs this feature needs that
// neither shared file exports. Kept local to `components/admin/**`.
import { Ban, CircleCheck, Trash2, UserPlus } from 'lucide-react';
import type { SVGProps } from 'react';

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

export function SuspendIcon({ size = 15, ...props }: IconProps) {
  return <Ban size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function ReactivateIcon({ size = 15, ...props }: IconProps) {
  return <CircleCheck size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function RemoveMemberIcon({ size = 15, ...props }: IconProps) {
  return <Trash2 size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}

export function InviteIcon({ size = 15, ...props }: IconProps) {
  return <UserPlus size={size} strokeWidth={2.75} aria-hidden="true" {...props} />;
}
