// P2-12 (plans/markhand-web/phase-2-web-spa.md §P2.6): usage/quota admin —
// renders `GET /usage`'s per-resource `{limit, committed, reserved,
// remaining}` snapshot. Read-only: no mutation on this page, so unlike
// `AdminMembersPage.tsx` there is no retained-data-across-refresh need beyond
// the one `useScopeSafeRequest` retry the error banner's own "Thử lại" uses.
import { useState } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import { UsageCards, describeMemberReadError } from '../components/admin';
import { Notice } from '../components/ui';
import { useScopeSafeRequest } from '../hooks/useScopeSafeRequest';

export function AdminUsagePage({ client = apiClient }: { client?: ApiClient } = {}) {
  const [retry, setRetry] = useState(0);
  const usageResult = useScopeSafeRequest(
    (signal) => client.request('get', '/usage', { signal }),
    [client, retry],
  );
  const items = usageResult.data?.items ?? [];

  return (
    <section className="page" aria-labelledby="admin-usage-heading">
      <p className="eyebrow">Quản trị</p>
      <h1 id="admin-usage-heading">Sử dụng và hạn mức</h1>
      <p className="lede">
        Tổng hợp sử dụng, hạn mức và phần đang được giữ chỗ (reservation) cho từng loại tài nguyên
        của tổ chức.
      </p>

      {usageResult.status === 'error' && (
        <Notice
          tone="error"
          action={
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => setRetry((n) => n + 1)}
            >
              Thử lại
            </button>
          }
        >
          {describeMemberReadError(usageResult.error)}
        </Notice>
      )}

      <UsageCards items={items} loading={usageResult.status === 'loading' && items.length === 0} />
    </section>
  );
}
