// The application shell's vertical left icon rail: brand mark, primary
// destinations, and the bottom cluster (org identity, account
// menu). See ../../../plans (P2.3) and the shell task brief for the
// requirements this satisfies; see styles.css's "rail" section for the
// layout notes (why the bottom cluster can never be clipped).
import { useState } from 'react';
import {
  CircleHelp,
  FolderKanban,
  Gauge,
  Library,
  MessageCircleQuestion,
  Network,
  PanelLeftClose,
  PanelLeftOpen,
  Users,
  type LucideIcon,
} from 'lucide-react';
import { useAuth } from '../../auth/AuthContext';
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
  // Expanded by default: labelled destinations beat icon-only mystery meat
  // (see the shell redesign notes). Only an explicit collapse ('0') persists.
  try {
    return window.localStorage.getItem(RAIL_EXPANDED_KEY) !== '0';
  } catch {
    return true;
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
  /**
   * UI convenience only, same caveat `RouteGuard.tsx`'s own `ProtectedRoute`
   * makes about its `permission` prop — hides the rail item when the
   * signed-in caller lacks it, never an authorization decision (the target
   * page's own `ProtectedRoute` and the server's 403 are what actually
   * decide). Omitted means "always shown", matching every destination below
   * before this field existed.
   */
  permission?: string;
}

/**
 * Primary destinations shown as rail icons. Deliberately excludes `login`
 * (public-only — the rail itself is not rendered on that route, see App.tsx)
 * and does not invent a separate "upload" destination: the router
 * (`types/routes.ts`) has no `/upload` route — the route list is
 * `/login`, `/library`, `/qa`, `/graph`, `/admin/projects`, `/admin/members`,
 * `/admin/usage`, `/help` — so "upload" from the shell brief lives inside
 * LibraryPage's own UI, not as a rail-level navigation target. `graph`
 * (P2-17, "Đồ thị") is the newest addition — a read-only cross-document
 * view, so it sits right after `qa` rather than grouped with the admin-only
 * destinations below it.
 */
const PRIMARY_DESTINATIONS: RailDestination[] = [
  { route: 'library', to: '/library', label: 'Thư viện', Icon: Library },
  { route: 'qa', to: '/qa', label: 'Hỏi đáp', Icon: MessageCircleQuestion },
  { route: 'graph', to: '/graph', label: 'Đồ thị', Icon: Network },
];

/**
 * "Khu Quản trị" (owner-approved rail design, 2026-07-29): "Dự án" (new,
 * P2-18's project management moved here from `LibraryPage`'s old
 * `ProjectsPanel`) grouped with the two pre-existing admin destinations
 * under one visible "QUẢN TRỊ" divider/label, rather than reading as three
 * unrelated icons in the middle of the rail. Only "Dự án" carries a
 * `permission` here — Members/Usage were never rail-gated (only their own
 * page content is, via `ProtectedRoute`'s `permission` prop in `App.tsx`)
 * and this change does not touch that. `doc.upload` is the same permission
 * `POST /projects`/`POST /collections/{id}/assign-project` require
 * server-side (see `AdminProjectsPage.tsx`'s own module doc) — no new
 * permission was invented for this move.
 */
const ADMIN_DESTINATIONS: RailDestination[] = [
  {
    route: 'adminProjects',
    to: '/admin/projects',
    label: 'Dự án',
    Icon: FolderKanban,
    permission: 'doc.upload',
  },
  { route: 'adminMembers', to: '/admin/members', label: 'Thành viên', Icon: Users },
  { route: 'adminUsage', to: '/admin/usage', label: 'Sử dụng', Icon: Gauge },
];

const HELP_DESTINATION: RailDestination = {
  route: 'help',
  to: '/help',
  label: 'Trợ giúp',
  Icon: CircleHelp,
};

function RailNavItem({ destination, active }: { destination: RailDestination; active: boolean }) {
  const { to, label, Icon } = destination;
  return (
    <li>
      <RailHint label={label}>
        <RouteLink
          to={to}
          className="rail-btn rail-link"
          aria-label={label}
          aria-current={active ? 'page' : undefined}
        >
          <Icon size={20} strokeWidth={2.75} aria-hidden="true" />
          <span className="rail-link-label" aria-hidden="true">
            {label}
          </span>
        </RouteLink>
      </RailHint>
    </li>
  );
}

export function Rail() {
  const { match } = useRouter();
  const { hasPermission } = useAuth();
  const [expanded, setExpanded] = useState(readExpandedPref);
  const visibleAdminDestinations = ADMIN_DESTINATIONS.filter(
    (destination) => !destination.permission || hasPermission(destination.permission),
  );

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
          {PRIMARY_DESTINATIONS.map((destination) => (
            <RailNavItem
              key={destination.route}
              destination={destination}
              active={match.name === destination.route}
            />
          ))}

          {/* "QUẢN TRỊ" group divider — visible label only in the expanded
              rail (mirrors `.rail-link-label`'s own collapsed/expanded
              split); `aria-hidden` because it is a purely visual grouping
              cue, not a landmark a screen-reader user needs announced
              (each item beneath it still has its own accessible name). */}
          <li className="rail-nav-divider" role="presentation" aria-hidden="true">
            <span className="rail-nav-group-label">Quản trị</span>
          </li>
          {visibleAdminDestinations.map((destination) => (
            <RailNavItem
              key={destination.route}
              destination={destination}
              active={match.name === destination.route}
            />
          ))}

          <RailNavItem destination={HELP_DESTINATION} active={match.name === 'help'} />
        </ul>
      </nav>

      <div className="rail-bottom">
        <OrgSwitch />
        <UserMenu />
      </div>
    </aside>
  );
}
