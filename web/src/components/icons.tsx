// `app/src/components/ui.tsx` (desktop) draws its icons from the `lucide-react`
// dependency. That package is not declared in web/package.json and this
// agent must not add a runtime dependency, so these are small hand-authored
// stand-ins with the same 24x24 stroke-icon shape (not a port of lucide's
// path data). Swap for a shared icon set if one is ever added to web.
import type { ReactNode, SVGProps } from 'react';

export type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function StrokeIcon({ size = 15, children, ...props }: IconProps & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}
    >
      {children}
    </svg>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <StrokeIcon {...props}>
      <path d="M20 6 9 17l-5-5" />
    </StrokeIcon>
  );
}

export function ChevronDownIcon(props: IconProps) {
  return (
    <StrokeIcon {...props}>
      <path d="m6 9 6 6 6-6" />
    </StrokeIcon>
  );
}

export function SpinnerIcon(props: IconProps) {
  return (
    <StrokeIcon {...props}>
      <path d="M12 3a9 9 0 1 0 9 9" />
    </StrokeIcon>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <StrokeIcon {...props}>
      <path d="m18 6-12 12" />
      <path d="m6 6 12 12" />
    </StrokeIcon>
  );
}
