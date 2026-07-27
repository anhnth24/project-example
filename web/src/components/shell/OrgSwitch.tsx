// The rail's "org switch" control.
//
// Honesty check before wiring this: `useScope()`'s `Scope` (state/scope.ts)
// carries exactly one `orgId` — there is no list of other orgs a session
// could move to anywhere in this client. `plans/markhand-web/phase-1c-multi-org-security.md`
// (P1C.1) *plans* an org list/detail + org switch/session-refresh API, but
// `api/generated/contract.ts` — generated straight from the server's actual
// OpenAPI doc — has no such endpoint yet, only `/auth/login`, `/auth/logout`,
// `/auth/me`, `/auth/refresh`. So there is nothing today for a person to
// switch *to*.
//
// Building a picker with one hard-coded entry (or fabricating fake orgs)
// would present a capability the backend doesn't have. Instead this shows
// real, current scope identity — org id, from the same `useScope()` seam the
// brief requires — in a popover, and says plainly that switching isn't wired
// yet. When P1C.1's org-list endpoint ships, this popover is the intended
// extension point: list the orgs, call the existing `useScope().setScope(...)`
// on selection (never a parallel path), and the abort/discard machinery in
// `state/scope.ts` already handles the rest.
import { Building2 } from 'lucide-react';
import { createPortal } from 'react-dom';
import { useScope } from '../../state/ScopeProvider';
import { RailHint } from './RailHint';
import { useRailPopover } from './useRailPopover';

export function OrgSwitch() {
  const { scope } = useScope();
  const { open, setOpen, triggerRef, menuRef, menuStyle } = useRailPopover(260);

  if (!scope) return null;

  const label = 'Đơn vị hiện tại';

  return (
    <>
      <RailHint label={label}>
        <button
          ref={triggerRef}
          type="button"
          className="btn btn-icon rail-btn"
          aria-label={label}
          aria-haspopup="dialog"
          aria-expanded={open}
          onClick={() => setOpen((current) => !current)}
        >
          <Building2 size={19} strokeWidth={2.75} aria-hidden="true" />
        </button>
      </RailHint>
      {open &&
        menuStyle &&
        createPortal(
          <div
            ref={menuRef}
            className="ui-select-menu rail-menu"
            role="dialog"
            aria-label={label}
            style={menuStyle}
          >
            <p className="rail-menu-kicker">{label}</p>
            <p className="rail-menu-org-id">org {scope.orgId}</p>
            <p className="rail-menu-note">
              Mỗi phiên hiện chỉ gắn với một đơn vị — chưa có API để liệt kê hoặc chuyển sang đơn vị
              khác.
            </p>
          </div>,
          document.body,
        )}
    </>
  );
}
