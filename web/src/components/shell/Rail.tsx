// The application shell's vertical left icon rail: brand mark, primary
// destinations, and the bottom cluster (org identity, account
// menu). See ../../../plans (P2.3) and the shell task brief for the
// requirements this satisfies; see styles.css's "rail" section for the
// layout notes (why the bottom cluster can never be clipped).
import { useState } from 'react';
import {
  CircleHelp,
  Gauge,
  Library,
  MessageCircleQuestion,
  Network,
  PanelLeftClose,
  PanelLeftOpen,
  Users,
  type LucideIcon,
} from 'lucide-react';
import { BrandMark } from '../BrandMark';
import { RouteLink } from '../RouteLink';
import { useRouter } from '../../state/RouterProvider';
import type { RouteName } from '../../types/routes';
import { OrgSwitch } from './OrgSwitch';
import { RailHint } from './RailHint';
import { UserMenu } from './UserMenu';

/**
 * localStorage key for the collapsed/expanded preference. Persisted so the
 * choice survives reloads, read defensively (private-mode / disabled storage
 * throws on access) so the rail always renders even if storage is unavailable.
 */
const RAIL_EXPANDED_KEY = 'markhand.rail.expanded';

function readExpandedPref(): boolean {
  try {
    return window.localStorage.getItem(RAIL_EXPANDED_KEY) === '1';
  } catch {
    return false;
  }
}

function writeExpandedPref(expanded: boolean): void {
  try {
    window.localStorage.setItem(RAIL_EXPANDED_KEY, expanded ? '1' : '0');
  } catch {
    // Storage unavailable (private mode, quota) — the preference simply
    // doesn't persist across reloads; the in-session toggle still works.
  }
}

interface RailDestination {
  route: RouteName;
  to: string;
  label: string;
  Icon: LucideIcon;
}

/**
 * Primary destinations shown as rail icons. Deliberately excludes `login`
 * (public-only — the rail itself is not rendered on that route, see App.tsx)
 * and does not invent a separate "upload" destination: the router
 * (`types/routes.ts`) has no `/upload` route — the route list is
 * `/login`, `/library`, `/qa`, `/graph`, `/admin/members`, `/admin/usage`,
 * `/help` — so "upload" from the shell brief lives inside LibraryPage's own
 * UI, not as a rail-level navigation target. `graph` (P2-17, "Đồ thị") is
 * the newest addition — a read-only cross-document view, so it sits right
 * after `qa` rather than grouped with the admin-only destinations below it.
 */
const RAIL_DESTINATIONS: RailDestination[] = [
  { route: 'library', to: '/library', label: 'Thư viện', Icon: Library },
  { route: 'qa', to: '/qa', label: 'Hỏi đáp', Icon: MessageCircleQuestion },
  { route: 'graph', to: '/graph', label: 'Đồ thị', Icon: Network },
  { route: 'adminMembers', to: '/admin/members', label: 'Thành viên', Icon: Users },
  { route: 'adminUsage', to: '/admin/usage', label: 'Sử dụng', Icon: Gauge },
  { route: 'help', to: '/help', label: 'Trợ giúp', Icon: CircleHelp },
];

export function Rail() {
  const { match } = useRouter();
  const [expanded, setExpanded] = useState(readExpandedPref);

  function toggleExpanded() {
    setExpanded((prev) => {
      const next = !prev;
      writeExpandedPref(next);
      return next;
    });
  }

  return (
    <aside
      className={`rail ${expanded ? 'rail-expanded' : ''}`}
      aria-label="Thanh điều hướng Folyvo"
    >
      <RailHint label="Trang chủ Folyvo">
        <RouteLink to="/" className="rail-brand" aria-label="Trang chủ Folyvo">
          <BrandMark className="rail-brand-mark" />
          <span className="rail-brand-word" aria-hidden="true">
            Folyvo
          </span>
        </RouteLink>
      </RailHint>

      <button
        type="button"
        className="rail-toggle"
        aria-pressed={expanded}
        aria-label={expanded ? 'Thu gọn thanh điều hướng' : 'Mở rộng thanh điều hướng'}
        onClick={toggleExpanded}
      >
        {expanded ? (
          <PanelLeftClose size={18} strokeWidth={2.75} aria-hidden="true" />
        ) : (
          <PanelLeftOpen size={18} strokeWidth={2.75} aria-hidden="true" />
        )}
      </button>

      <nav className="rail-nav" aria-label="Điều hướng chính">
        <ul className="rail-nav-list">
          {RAIL_DESTINATIONS.map(({ route, to, label, Icon }) => (
            <li key={route}>
              <RailHint label={label}>
                <RouteLink
                  to={to}
                  className="rail-btn rail-link"
                  aria-label={label}
                  aria-current={match.name === route ? 'page' : undefined}
                >
                  <Icon size={20} strokeWidth={2.75} aria-hidden="true" />
                  <span className="rail-link-label" aria-hidden="true">
                    {label}
                  </span>
                </RouteLink>
              </RailHint>
            </li>
          ))}
        </ul>
      </nav>

      <div className="rail-bottom">
        <OrgSwitch />
        <UserMenu />
      </div>
    </aside>
  );
}
