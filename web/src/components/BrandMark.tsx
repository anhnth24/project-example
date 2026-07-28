// The Folyvo brand mark (source: app/assets/folyvo-logo-icon/app-icon.svg —
// charcoal rounded-square badge with the orange "F"). Rendered as an <img> so
// the vector ships as a hashed static asset rather than being inlined into
// every component that shows it. Purely decorative — the surrounding brand
// link/heading already carries the accessible name — so it is aria-hidden with
// an empty alt.
import brandIcon from '../assets/brand/folyvo-app-icon.svg';

export function BrandMark({ className }: { className?: string }) {
  return (
    <img
      src={brandIcon}
      alt=""
      aria-hidden="true"
      className={className}
      width={44}
      height={44}
      draggable={false}
    />
  );
}
