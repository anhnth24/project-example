// The rail's org switcher (1C-01 + P2-06/P2-15 org switch).
//
// `GET /orgs` lists every org the caller is currently an active member of;
// picking a different one calls `useAuth().switchOrg(orgId)` — never a
// parallel path — which does the atomic token swap + scope-epoch bump (see
// that method's own doc in `auth/AuthContext.tsx`) and, through the epoch
// machinery every `useScopeSafeRequest`/`useScopeSafeSse` caller already
// registers with, aborts every in-flight request for the old org and makes
// the next render fetch fresh under the new one. Once the switch and its
// `useAuth()` promise resolve, this component closes the popover and
// navigates home (`/`) — the "điều hướng về trạng thái đầu org mới" the task
// brief asks for — rather than staying on a route (or a `?collectionId=`)
// that named something specific to the org just left.
//
// A denied/network/rate-limited switch never touches the session/scope (see
// `switchOrg`'s own contract) — this component shows the error inline and
// leaves the popover open on the still-current org, exactly the "lỗi switch
// hiển thị accessible, không đổi scope" the brief asks for.
import { Building2, Check } from 'lucide-react';
import { useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { apiClient, HttpApiError, NetworkError, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { useAuth } from '../../auth/AuthContext';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { useRouter } from '../../state/RouterProvider';
import { useScope } from '../../state/ScopeProvider';
import { RailHint } from './RailHint';
import { useRailPopover } from './useRailPopover';

type Org = components['schemas']['Org'];

const ROLE_LABELS: Record<Org['role'], string> = {
  owner: 'Chủ sở hữu',
  admin: 'Quản trị viên',
  editor: 'Biên tập viên',
  viewer: 'Người xem',
};

/**
 * Vietnamese, user-facing message for a failed `switchOrg` call. Deliberately
 * its own helper, not `components/library`'s `describeApiError`: that one's
 * 403/404 wording is specific to collections/documents, and a 403 here always
 * means the real server's `membership_missing` (see `mocks/handlers/orgs.ts`
 * / `crates/server/src/auth/session.rs`) — "no longer a member", not "no
 * permission".
 */
function describeSwitchError(error: unknown): string {
  if (error instanceof HttpApiError) {
    if (error.status === 403) {
      return 'Bạn không còn là thành viên của đơn vị này. Danh sách sẽ được làm mới.';
    }
    if (error.status === 429) return 'Quá nhiều yêu cầu. Vui lòng thử lại sau ít phút.';
    return `Máy chủ báo lỗi (${error.status}): ${error.message}`;
  }
  if (error instanceof NetworkError) {
    return 'Không thể kết nối máy chủ. Kiểm tra kết nối mạng và thử lại.';
  }
  return 'Không thể chuyển đơn vị lúc này. Vui lòng thử lại.';
}

export interface OrgSwitchProps {
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `LibraryPage`/`AdminMembersPage`. */
  client?: ApiClient;
}

export function OrgSwitch({ client = apiClient }: OrgSwitchProps) {
  const { scope } = useScope();
  const { switchOrg } = useAuth();
  const { navigate } = useRouter();
  const { open, setOpen, triggerRef, menuRef, menuStyle } = useRailPopover(280);

  // Re-fetches on mount and on every scope-epoch change (login, logout, a
  // completed switch) — always the org list for whoever is signed in *now*,
  // never a stale one left over from a previous session/org. `scope` is
  // read inside the callback (not awaited-around) since a null scope simply
  // means "nothing to list yet" rather than an error.
  const orgsResult = useScopeSafeRequest<Org[]>(async (signal) => {
    if (!scope) return [];
    const page = await client.request('get', '/orgs', { signal });
    return page.items;
  }, []);

  const [pendingOrgId, setPendingOrgId] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState<string | null>(null);
  // UI-feedback-only "latest click wins" ticket: `useAuth().switchOrg` itself
  // already guarantees the SESSION/SCOPE ends up correct under a rapid
  // double-switch (its own epoch guard, see that method's doc) even without
  // this — this ref only stops an older click's `catch` from painting a
  // stale error/spinner over a newer click's still-pending or already-settled UI state.
  const ticketRef = useRef(0);

  if (!scope) return null;

  const label = 'Đơn vị hiện tại';

  async function handleSwitch(orgId: string) {
    if (orgId === scope?.orgId || pendingOrgId !== null) return;
    const ticket = (ticketRef.current += 1);
    setPendingOrgId(orgId);
    setSwitchError(null);
    try {
      await switchOrg(orgId);
      if (ticketRef.current !== ticket) return;
      setPendingOrgId(null);
      setOpen(false);
      navigate('/'); // "điều hướng về trạng thái đầu org mới" — never a route/param naming something specific to the org just left.
    } catch (cause) {
      if (ticketRef.current !== ticket) return;
      setPendingOrgId(null);
      setSwitchError(describeSwitchError(cause));
    }
  }

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

            {orgsResult.status === 'loading' && (
              <p className="rail-menu-note" role="status">
                Đang tải danh sách đơn vị…
              </p>
            )}

            {orgsResult.status === 'error' && (
              <p className="rail-menu-note" role="alert">
                Không thể tải danh sách đơn vị. Vui lòng thử lại.
              </p>
            )}

            {orgsResult.status === 'success' && (
              <ul className="rail-menu-org-list" aria-label="Danh sách đơn vị">
                {(orgsResult.data ?? []).map((org) => {
                  const isCurrent = org.id === scope.orgId;
                  const isPending = pendingOrgId === org.id;
                  return (
                    <li key={org.id}>
                      <button
                        type="button"
                        className={`ui-select-option rail-menu-org-option ${isCurrent ? 'active' : ''}`}
                        disabled={isCurrent || pendingOrgId !== null}
                        aria-current={isCurrent ? 'true' : undefined}
                        onClick={() => void handleSwitch(org.id)}
                      >
                        <span className="rail-menu-org-name">
                          <span>{org.name}</span>
                          <span className="rail-menu-org-role">{ROLE_LABELS[org.role]}</span>
                        </span>
                        {isCurrent && <Check size={14} aria-hidden="true" />}
                        {isPending && (
                          <span className="rail-menu-org-pending" role="status">
                            Đang chuyển…
                          </span>
                        )}
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}

            {switchError && (
              <p className="notice notice-error rail-menu-switch-error" role="alert">
                {switchError}
              </p>
            )}
          </div>,
          document.body,
        )}
    </>
  );
}
