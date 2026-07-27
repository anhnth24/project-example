// The rail's account control.
//
// The brief lists four bottom-cluster items: "theme toggle, user, org
// switch, and the user avatar". Both design sources (the v2 export and the
// interactive prototype) implement "user" and "the user avatar" as the exact
// same element — a single avatar-shaped button that opens the account menu
// (v2's `toggleUserMenu`/`userMenu` at the button showing initials; the
// prototype's `.user-avatar` button, `aria-label="Tài khoản ..."`). Rendering
// two icon-only buttons side by side for the same referent would be a
// redundant, unlabelable pair, not two features — so this file treats "user"
// and "the user avatar" as one control: the avatar *is* the user-menu
// trigger, matching what both sources actually build.
//
// Session/org/permission visibility and the logout call are carried over
// unchanged from the previous topbar's `AuthStatus` (App.tsx, pre-rail): same
// `useAuth().logout()` + `navigate('/login')` sequence, same permissions
// list, just moved into a popover instead of a `title` tooltip (a11y: a
// visible/focusable panel beats text hidden in a native tooltip).
import { LogOut } from 'lucide-react';
import { useState } from 'react';
import { createPortal } from 'react-dom';
import { useAuth } from '../../auth/AuthContext';
import { useRouter } from '../../state/RouterProvider';
import { RailHint } from './RailHint';
import { useRailPopover } from './useRailPopover';

function initialsFor(displayName: string): string {
  const words = displayName.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return '?';
  if (words.length === 1) return words[0].slice(0, 2).toUpperCase();
  return (words[0][0] + words[words.length - 1][0]).toUpperCase();
}

export function UserMenu() {
  const { session, logout } = useAuth();
  const { navigate } = useRouter();
  const { open, setOpen, triggerRef, menuRef, menuStyle } = useRailPopover(240);
  const [loggingOut, setLoggingOut] = useState(false);

  if (session.status !== 'authenticated') return null;

  const label = `Tài khoản: ${session.displayName}`;
  const permissionsCopy = session.permissions.length
    ? session.permissions.join(', ')
    : '(không có)';

  async function handleLogout() {
    setLoggingOut(true);
    setOpen(false);
    try {
      await logout();
    } finally {
      setLoggingOut(false);
      navigate('/login');
    }
  }

  return (
    <>
      <RailHint label={label}>
        <button
          ref={triggerRef}
          type="button"
          className="rail-avatar"
          aria-label={label}
          aria-haspopup="dialog"
          aria-expanded={open}
          onClick={() => setOpen((current) => !current)}
        >
          {initialsFor(session.displayName)}
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
            <p className="rail-menu-kicker">Tài khoản</p>
            <p className="rail-menu-row rail-menu-name">{session.displayName}</p>
            <p className="rail-menu-note">Quyền: {permissionsCopy}</p>
            <button
              type="button"
              className="ui-select-option rail-menu-logout"
              disabled={loggingOut}
              onClick={() => void handleLogout()}
            >
              <LogOut size={15} strokeWidth={2.75} aria-hidden="true" />
              <span>{loggingOut ? 'Đang đăng xuất…' : 'Đăng xuất'}</span>
            </button>
          </div>,
          document.body,
        )}
    </>
  );
}
