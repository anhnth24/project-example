// P2-11: create-invite form. `POST /members/invites` returns the plaintext
// token exactly once (see the OpenAPI doc's `CreateInviteResponse.token`
// comment) — this component is the only place that ever sees it, holds it in
// local state only for as long as the confirmation modal is open, and never
// re-fetches or persists it anywhere (not in `mocks/fixtures.ts`'s store
// either — see `handlers/members.ts`, which only ever stores a hash).
import { useId, useState, type FormEvent } from 'react';
import { Button, Modal, Notice, SelectControl, type SelectOption } from '../ui';
import { createInvite } from './membersApi';
import { ROLE_ORDER, ROLE_META, describeMemberActionError } from './memberPresentation';
import { InviteIcon } from './icons';
import { useSingleFlightAction } from '../actions/useSingleFlightAction';
import { apiClient, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import type { MembershipRole } from './types';

type CreateInviteResponse = components['schemas']['CreateInviteResponse'];

const TTL_PRESETS: Array<{ value: string; label: string; secs: number | undefined }> = [
  { value: 'default', label: 'Mặc định (7 ngày)', secs: undefined },
  { value: 'day', label: '1 ngày', secs: 24 * 3600 },
  { value: 'month', label: '30 ngày', secs: 30 * 24 * 3600 },
];

export function InviteForm({
  isOwnerActive,
  onCreated,
  client = apiClient,
}: {
  isOwnerActive: boolean;
  /** Called once the invite is created (server-confirmed), so the caller can refetch the invites list. Never carries the token — that only ever lives in this component's own confirmation modal. */
  onCreated: () => void;
  client?: ApiClient;
}) {
  const emailId = useId();
  const [email, setEmail] = useState('');
  const [role, setRole] = useState<MembershipRole>('viewer');
  const [ttlChoice, setTtlChoice] = useState('default');
  const [issued, setIssued] = useState<CreateInviteResponse | null>(null);

  const action = useSingleFlightAction<CreateInviteResponse>();
  const pending = action.phase === 'pending';

  const roleOptions: SelectOption[] = ROLE_ORDER.map((r) => ({
    value: r,
    label: ROLE_META[r].label,
    disabled: r === 'owner' && !isOwnerActive,
  }));

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const trimmed = email.trim();
    if (!trimmed) return;
    const ttlSecs = TTL_PRESETS.find((p) => p.value === ttlChoice)?.secs;
    const started = action.dispatch('create-invite', (signal) =>
      createInvite({ client, email: trimmed, role, ttlSecs, signal }),
    );
    if (started) setIssued(null);
  }

  // Adjust-state-while-rendering (same idiom `useScopeSafeRequest`'s own doc
  // names): once the ticket settles successfully, capture its one-time token
  // into `issued` for the modal below, and tell the caller to refetch the
  // invites list. Not an effect: this only needs to run once, exactly when
  // `action.phase` transitions to `success`, and comparing against `issued`
  // directly avoids a second `useEffect` + ref just to track "already
  // handled this ticket".
  if (action.phase === 'success' && action.value && issued !== action.value) {
    setIssued(action.value);
    setEmail('');
    onCreated();
  }

  function closeTokenModal() {
    setIssued(null);
    action.reset();
  }

  return (
    <div>
      <form
        className="auth-form"
        style={{
          // Override `.auth-form`'s column + 24rem cap: the invite form is a
          // horizontal row of fields (email · role · TTL · submit) that wraps
          // only when it runs out of width, not a stacked login column.
          display: 'flex',
          flexDirection: 'row',
          flexWrap: 'wrap',
          alignItems: 'flex-end',
          gap: 'var(--space-3)',
          maxWidth: 'none',
        }}
        onSubmit={handleSubmit}
        noValidate
      >
        <div className="field" style={{ flex: '1 1 220px' }}>
          <label htmlFor={emailId}>Email người được mời</label>
          <input
            id={emailId}
            name="email"
            type="email"
            required
            disabled={pending}
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
        </div>

        <div className="field" style={{ minWidth: 180 }}>
          <span className="text-muted">Vai trò</span>
          <SelectControl
            value={role}
            options={roleOptions}
            onChange={(value) => setRole(value as MembershipRole)}
            ariaLabel="Vai trò cho lời mời"
            disabled={pending}
          />
        </div>

        <div className="field" style={{ minWidth: 180 }}>
          <span className="text-muted">Thời hạn lời mời</span>
          <SelectControl
            value={ttlChoice}
            options={TTL_PRESETS.map((p) => ({ value: p.value, label: p.label }))}
            onChange={setTtlChoice}
            ariaLabel="Thời hạn lời mời"
            disabled={pending}
          />
        </div>

        <Button
          type="submit"
          variant="primary"
          icon={<InviteIcon />}
          loading={pending}
          disabled={pending}
        >
          Gửi lời mời
        </Button>
      </form>

      {action.phase === 'error' && (
        <Notice tone="error">{describeMemberActionError(action.error, role === 'owner')}</Notice>
      )}

      {issued && (
        <Modal
          title="Đã tạo lời mời"
          description="Mã mời chỉ hiển thị một lần duy nhất ngay bây giờ — hãy sao chép và gửi cho người được mời. Folyvo sẽ không thể hiển thị lại mã này sau khi đóng hộp thoại."
          onClose={closeTokenModal}
          footer={
            <Button variant="primary" onClick={closeTokenModal}>
              Đã sao chép, đóng lại
            </Button>
          }
        >
          <p>
            Email: <strong>{issued.invite.email}</strong> — vai trò:{' '}
            <span className={`tag ${ROLE_META[issued.invite.role].tagClass}`}>
              {ROLE_META[issued.invite.role].label}
            </span>
          </p>
          <div className="field">
            <label htmlFor={`${emailId}-token`}>Mã mời (một lần)</label>
            <input
              id={`${emailId}-token`}
              readOnly
              value={issued.token}
              onFocus={(e) => e.target.select()}
            />
          </div>
          <Button
            type="button"
            variant="secondary"
            onClick={() => void navigator.clipboard?.writeText(issued.token)}
          >
            Sao chép mã mời
          </Button>
        </Modal>
      )}
    </div>
  );
}
