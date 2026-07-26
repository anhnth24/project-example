// Internal navigation link for the hand-rolled router in `../state/RouterProvider`.
// Renders a real `<a href>` (so middle-click/open-in-new-tab and screen
// reader landmark navigation keep working) but intercepts plain left-clicks
// to go through `navigate` instead of a full page reload.
import type { AnchorHTMLAttributes, MouseEvent } from 'react';
import { useRouter } from '../state/RouterProvider';

export function RouteLink({
  to,
  onClick,
  ...props
}: { to: string } & AnchorHTMLAttributes<HTMLAnchorElement>) {
  const { navigate } = useRouter();

  function handleClick(event: MouseEvent<HTMLAnchorElement>) {
    onClick?.(event);
    if (
      event.defaultPrevented ||
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey
    ) {
      return;
    }
    event.preventDefault();
    navigate(to);
  }

  return <a href={to} onClick={handleClick} {...props} />;
}
