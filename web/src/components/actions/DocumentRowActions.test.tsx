import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { StrictMode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import {
  getStore,
  installMockFetch,
  mockControl,
  resetMockState,
  uninstallMockFetch,
} from '../../mocks';
import { createScopeManager, type ScopeManager } from '../../state/scope';
import { ScopeProvider } from '../../state/ScopeProvider';
import { DocumentRowActions } from './DocumentRowActions';
import { triggerBrowserDownload } from './saveBlob';
import { signInDemoUser } from './testSupport';

type Document = components['schemas']['Document'];

vi.mock('./saveBlob', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./saveBlob')>();
  return { ...actual, triggerBrowserDownload: vi.fn() };
});

function makeDocument(overrides: Partial<Document> = {}): Document {
  const store = getStore();
  const [collection] = store.collections;
  return {
    // Real (random, RFC 4122-shaped) uuids, not readable placeholder
    // strings: the mock's spec-drift guard validates every response body
    // against the OpenAPI schema, including `format: uuid` fields the
    // handlers echo straight back (e.g. reindexDocument's `documentId`) —
    // a non-uuid id fails that validation and surfaces as a NetworkError,
    // which is a red herring for whatever the test is actually checking.
    id: crypto.randomUUID(),
    collectionId: collection.id,
    title: 'Hop dong dich vu 2026.docx',
    state: 'indexed',
    currentVersionId: crypto.randomUUID(),
    createdAt: '2026-01-01T00:00:00.000Z',
    updatedAt: '2026-01-01T00:00:00.000Z',
    ...overrides,
  };
}

/** Seeds a document (and one current version) directly into the mock store under the demo user's first collection, so the real handlers (issue/redeem/reindex/delete) operate on it. */
function seedDocument(overrides: Partial<Document> = {}): Document {
  const store = getStore();
  const doc = makeDocument(overrides);
  const docs = store.documents.get(doc.collectionId) ?? [];
  docs.push(doc);
  store.documents.set(doc.collectionId, docs);
  if (doc.currentVersionId) {
    store.versions.set(doc.id, [
      {
        id: doc.currentVersionId,
        documentId: doc.id,
        versionNumber: 1,
        isCurrent: true,
        sourceContentSha256: 'x'.repeat(64),
        effectiveFrom: doc.createdAt,
        effectiveTo: null,
        changeSummary: null,
        createdAt: doc.createdAt,
      },
    ]);
  }
  return doc;
}

function renderRow(doc: Document, opts: { onChanged?: () => void; strict?: boolean } = {}) {
  const manager: ScopeManager = createScopeManager();
  manager.setScope({ orgId: 'org-1', permissions: [], allowedCollectionIds: [doc.collectionId] });
  const tree = (
    <ScopeProvider manager={manager}>
      <DocumentRowActions document={doc} onChanged={opts.onChanged} />
    </ScopeProvider>
  );
  return render(opts.strict ? <StrictMode>{tree}</StrictMode> : tree);
}

beforeEach(() => {
  installMockFetch();
  resetMockState();
  signInDemoUser();
});

afterEach(() => {
  cleanup();
  uninstallMockFetch();
  apiClient.sessionManager.clear();
  vi.restoreAllMocks();
  // `vi.mock('./saveBlob', ...)` builds a `vi.fn()` once at module-eval
  // time, not per test — `restoreAllMocks` only restores `vi.spyOn` spies
  // to their original implementation, it does not reset a mock's call
  // history, so `triggerBrowserDownload`'s call log would otherwise leak
  // across `it()` blocks in this file.
  vi.clearAllMocks();
});

describe('DocumentRowActions', () => {
  it('renders the three row actions', () => {
    renderRow(seedDocument());
    expect(screen.getByRole('button', { name: 'Tải xuống' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Lập chỉ mục lại' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Xóa' })).toBeInTheDocument();
  });

  it('disables download and reindex/delete once the document is tombstoned/purged', () => {
    renderRow(seedDocument({ state: 'tombstoned' }));
    expect(screen.getByRole('button', { name: 'Tải xuống' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Lập chỉ mục lại' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Xóa' })).toBeDisabled();
  });

  it('disables download (but not reindex) when there is no current version yet', () => {
    renderRow(seedDocument({ state: 'converting', currentVersionId: null }));
    expect(screen.getByRole('button', { name: 'Tải xuống' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Lập chỉ mục lại' })).toBeEnabled();
  });

  it('labels the reindex action as retry when the document previously failed', () => {
    renderRow(seedDocument({ state: 'failed' }));
    expect(screen.getByRole('button', { name: 'Thử lại lập chỉ mục' })).toBeInTheDocument();
    expect(screen.getByText(/chưa có API thử lại chuyển đổi riêng/i)).toBeInTheDocument();
  });

  describe('download', () => {
    it('issues a capability, redeems it exactly once, and hands bytes to the browser', async () => {
      const doc = seedDocument();
      renderRow(doc);

      fireEvent.click(screen.getByRole('button', { name: 'Tải xuống' }));
      fireEvent.click(screen.getByRole('menuitem', { name: 'Markdown (.md)' }));

      await waitFor(() => expect(triggerBrowserDownload).toHaveBeenCalledOnce());
      expect(vi.mocked(triggerBrowserDownload).mock.calls[0][1]).toBe('Hop dong dich vu 2026.md');

      // Single-use: exactly one capability was minted and it is now redeemed.
      const capabilities = [...getStore().downloadCapabilities.values()];
      expect(capabilities).toHaveLength(1);
      expect(capabilities[0].redeemed).toBe(true);
    });

    it('never redeems twice under React 18 StrictMode (which double-invokes mount effects)', async () => {
      const doc = seedDocument();
      renderRow(doc, { strict: true });

      fireEvent.click(screen.getByRole('button', { name: 'Tải xuống' }));
      fireEvent.click(screen.getByRole('menuitem', { name: 'Tệp gốc' }));

      await waitFor(() => expect(triggerBrowserDownload).toHaveBeenCalledOnce());
      const capabilities = [...getStore().downloadCapabilities.values()];
      expect(capabilities).toHaveLength(1);
      expect(capabilities[0].redeemed).toBe(true);
    });

    it('shows a distinct message on 403 and does not attempt to save anything', async () => {
      mockControl.forceStatus('issueDownloadCapability', 403, { times: 1 });
      renderRow(seedDocument());

      fireEvent.click(screen.getByRole('button', { name: 'Tải xuống' }));
      fireEvent.click(screen.getByRole('menuitem', { name: 'Markdown (.md)' }));

      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(/không có quyền/i);
      expect(triggerBrowserDownload).not.toHaveBeenCalled();
    });

    it('shows a distinct message on 429', async () => {
      mockControl.forceStatus('issueDownloadCapability', 429, { times: 1 });
      renderRow(seedDocument());

      fireEvent.click(screen.getByRole('button', { name: 'Tải xuống' }));
      fireEvent.click(screen.getByRole('menuitem', { name: 'Markdown (.md)' }));

      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(/quá nhiều yêu cầu/i);
    });

    it('closes the format menu on outside click without dispatching anything', () => {
      renderRow(seedDocument());
      fireEvent.click(screen.getByRole('button', { name: 'Tải xuống' }));
      expect(screen.getByRole('menu')).toBeInTheDocument();

      fireEvent.pointerDown(document.body);
      expect(screen.queryByRole('menu')).not.toBeInTheDocument();
      expect(triggerBrowserDownload).not.toHaveBeenCalled();
    });
  });

  describe('reindex', () => {
    it('reports a freshly created job honestly', async () => {
      const doc = seedDocument();
      renderRow(doc);

      fireEvent.click(screen.getByRole('button', { name: 'Lập chỉ mục lại' }));

      await screen.findByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.');
    });

    it('reports an idempotent replay as "already running" rather than a new job', async () => {
      // The shared mock's `reindexDocument` handler always answers
      // `created: true` (see web/src/mocks/handlers/library.ts — it mints a
      // fresh job every call with no idempotency memoization), so it cannot
      // produce a `created: false` reply to exercise this against. Stubbing
      // `apiClient.request` for this one call is a narrower substitute that
      // still exercises this component's own rendering of that response
      // shape, without touching `mocks/**`.
      const doc = seedDocument();
      const requestSpy = vi.spyOn(apiClient, 'request').mockResolvedValueOnce({
        jobId: 'existing-job',
        created: false,
        documentId: doc.id,
        versionId: doc.currentVersionId,
        requestId: 'req-1',
      } as Awaited<ReturnType<typeof apiClient.request>>);

      renderRow(doc);
      fireEvent.click(screen.getByRole('button', { name: 'Lập chỉ mục lại' }));

      await screen.findByText(/đã có một tác vụ lập chỉ mục đang chạy/i);
      expect(requestSpy).toHaveBeenCalledWith(
        'post',
        '/documents/{documentId}/reindex',
        expect.objectContaining({ params: { path: { documentId: doc.id } } }),
      );
    });

    it('calls onChanged after a successful reindex', async () => {
      const onChanged = vi.fn();
      renderRow(seedDocument(), { onChanged });

      fireEvent.click(screen.getByRole('button', { name: 'Lập chỉ mục lại' }));
      await waitFor(() => expect(onChanged).toHaveBeenCalledOnce());
    });

    it('shows a distinct message on 429', async () => {
      mockControl.forceStatus('reindexDocument', 429, { times: 1 });
      renderRow(seedDocument());

      fireEvent.click(screen.getByRole('button', { name: 'Lập chỉ mục lại' }));
      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(/quá nhiều yêu cầu/i);
    });

    it('cannot be double-fired by two rapid clicks', async () => {
      const doc = seedDocument();
      renderRow(doc);

      const button = screen.getByRole('button', { name: 'Lập chỉ mục lại' });
      fireEvent.click(button);
      fireEvent.click(button); // same-tick second click while the first is still pending

      await screen.findByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.');
      // Only one job should have been enqueued for this document.
      const jobs = [...getStore().jobs.values()].filter((j) => j.documentId === doc.id);
      expect(jobs).toHaveLength(1);
    });
  });

  describe('delete', () => {
    it('requires confirmation before calling the API', () => {
      renderRow(seedDocument());
      fireEvent.click(screen.getByRole('button', { name: 'Xóa' }));

      expect(screen.getByRole('dialog', { name: 'Xóa tài liệu này?' })).toBeInTheDocument();
      // Cancel: no request should have fired, dialog closes.
      fireEvent.click(screen.getByRole('button', { name: 'Hủy' }));
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    });

    it('shows an asynchronous tombstone state after confirming, and calls onChanged', async () => {
      const onChanged = vi.fn();
      const doc = seedDocument();
      renderRow(doc, { onChanged });

      fireEvent.click(screen.getByRole('button', { name: 'Xóa' }));
      const dialog = screen.getByRole('dialog', { name: 'Xóa tài liệu này?' });
      fireEvent.click(within(dialog).getByRole('button', { name: 'Xóa tài liệu' }));

      await screen.findByText(/đã yêu cầu xóa tài liệu này/i);
      // onChanged fires from a passive effect after the success render commits,
      // so it may land a tick after the message is visible.
      await waitFor(() => expect(onChanged).toHaveBeenCalledOnce());
      // The row does not vanish by itself — deletion is asynchronous, the
      // parent decides when to stop rendering it (via onChanged's refetch).
      expect(screen.getByRole('button', { name: 'Xóa' })).toBeDisabled();
    });

    it('shows a distinct message on 429', async () => {
      mockControl.forceStatus('deleteDocument', 429, { times: 1 });
      renderRow(seedDocument());

      fireEvent.click(screen.getByRole('button', { name: 'Xóa' }));
      const dialog = screen.getByRole('dialog', { name: 'Xóa tài liệu này?' });
      fireEvent.click(within(dialog).getByRole('button', { name: 'Xóa tài liệu' }));

      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(/quá nhiều yêu cầu/i);
    });
  });

  it('disables every other action while one is pending', async () => {
    const doc = seedDocument();
    renderRow(doc);

    fireEvent.click(screen.getByRole('button', { name: 'Lập chỉ mục lại' }));
    // Still within the same synchronous act() batch as the click above —
    // the download/delete buttons must already be disabled.
    expect(screen.getByRole('button', { name: 'Tải xuống' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Xóa' })).toBeDisabled();

    await screen.findByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.');
    expect(screen.getByRole('button', { name: 'Tải xuống' })).toBeEnabled();
  });
});
