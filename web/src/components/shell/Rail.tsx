// The application shell's vertical left icon rail: brand mark, primary
// destinations, and the bottom cluster (theme toggle, org identity, account
// menu). See ../../../plans (P2.3) and the shell task brief for the
// requirements this satisfies; see styles.css's "rail" section for the
// layout notes (why the bottom cluster can never be clipped).
import {
  CircleHelp,
  Gauge,
  Library,
  MessageCircleQuestion,
  Users,
  type LucideIcon,
} from 'lucide-react';
import { RouteLink } from '../RouteLink';
import { useRouter } from '../../state/RouterProvider';
import type { RouteName } from '../../types/routes';
import { OrgSwitch } from './OrgSwitch';
import { RailHint } from './RailHint';
import { ThemeToggle } from './ThemeToggle';
import { UserMenu } from './UserMenu';

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
 * (`types/routes.ts`) has no `/upload` route — the P2.3 route list is
 * `/login`, `/library`, `/qa`, `/admin/members`, `/admin/usage`, `/help` —
 * so "upload" from the shell brief lives inside LibraryPage's own UI, not as
 * a rail-level navigation target.
 */
const RAIL_DESTINATIONS: RailDestination[] = [
  { route: 'library', to: '/library', label: 'Thư viện', Icon: Library },
  { route: 'qa', to: '/qa', label: 'Hỏi đáp', Icon: MessageCircleQuestion },
  { route: 'adminMembers', to: '/admin/members', label: 'Thành viên', Icon: Users },
  { route: 'adminUsage', to: '/admin/usage', label: 'Sử dụng', Icon: Gauge },
  { route: 'help', to: '/help', label: 'Trợ giúp', Icon: CircleHelp },
];

export function Rail() {
  const { match } = useRouter();

  return (
    <aside className="rail" aria-label="Thanh điều hướng Markhand">
      <RailHint label="Trang chủ Markhand">
        <RouteLink to="/" className="rail-brand" aria-label="Trang chủ Markhand">
          <span aria-hidden="true">M</span>
        </RouteLink>
      </RailHint>

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
                </RouteLink>
              </RailHint>
            </li>
          ))}
        </ul>
      </nav>

      <div className="rail-bottom">
        <ThemeToggle />
        <OrgSwitch />
        <UserMenu />
      </div>
    </aside>
  );
}
