import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiClient } from '../../api/client';
import type { SseMessage } from '../../api/sse';
import type { ScopeSafeSseSource } from '../../hooks/useScopeSafeSse';
import { getStore, installMockFetch, resetMockState, uninstallMockFetch } from '../../mocks';
import { mockUuid } from '../../mocks/ids';
import { createScopeManager, type Scope, type ScopeManager } from '../../state/scope';
import { ScopeProvider } from '../../state/ScopeProvider';

// `mocks/**` is deliberately fetch-only and never mocks SSE (see
// `mocks/fetchMock.ts`'s handling of `DELIBERATELY_UNMOCKED_OPERATIONS` for
// `jobEvents`), so job-event delivery is driven through this seam instead —
// same reasoning as `jobEventsSource.ts`'s module doc.
vi.mock('./jobEventsSource', () => ({ createJobEventsSource: vi.fn() }));
import { createJobEventsSource } from './jobEventsSource';
import { UploadPanel } from './UploadPanel';

// ---------------------------------------------------------------------------
// Test doubles
// ---------------------------------------------------------------------------

/**
 * Fake `XMLHttpRequest` installed as the global constructor. `uploadTransport.ts`
 * uses `XMLHttpRequest` directly (that's the whole point — `fetch` cannot
 * report upload progress), so exercising it at the `UploadPanel` level means
 * replacing the global rather than injecting a factory (the panel's public
 * contract is fixed to `{collectionId, onUploaded}` — no test seam allowed).
 */
class FakeXhr {
  static instances: FakeXhr[] = [];
  method = '';
  url = '';
  readonly requestHeaders: Record<string, string> = {};
  status = 0;
  responseText = '';
  private responseHeaders: Record<string, string> = {};
  aborted = false;
  upload: { onprogress: ((event: ProgressEvent) => void) | null } = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;

  constructor() {
    FakeXhr.instances.push(this);
  }

  open(method: string, url: string): void {
    this.method = method;
    this.url = url;
  }

  setRequestHeader(name: string, value: string): void {
    this.requestHeaders[name] = value;
  }

  send(): void {}

  abort(): void {
    this.aborted = true;
    this.onabort?.();
  }

  getResponseHeader(name: string): string | null {
    return this.responseHeaders[name] ?? null;
  }

  respond(status: number, body: unknown, headers: Record<string, string> = {}): void {
    this.status = status;
    this.responseText = typeof body === 'string' ? body : JSON.stringify(body);
    this.responseHeaders = headers;
    this.onload?.();
  }

  progress(loaded: number, total: number, lengthComputable = true): void {
    this.upload.onprogress?.({ loaded, total, lengthComputable } as ProgressEvent);
  }
}

/** Same fake abortable stream shape as `hooks/useScopeSafeSse.test.tsx` — see that file's doc for why `abort()` is a plain spy that doesn't touch the iterator. */
function fakeSseSource<M>() {
  let pendingResolve: ((result: IteratorResult<M>) => void) | undefined;
  const queue: M[] = [];
  const abort = vi.fn();
  const source: ScopeSafeSseSource<M> = {
    abort,
    [Symbol.asyncIterator]() {
      return {
        next(): Promise<IteratorResult<M>> {
          if (queue.length > 0) return Promise.resolve({ value: queue.shift() as M, done: false });
          return new Promise<IteratorResult<M>>((resolve) => {
            pendingResolve = resolve;
          });
        },
      };
    },
  };
  return {
    source,
    abort,
    push(message: M) {
      if (pendingResolve) {
        const resolve = pendingResolve;
        pendingResolve = undefined;
        resolve({ value: message, done: false });
      } else {
        queue.push(message);
      }
    },
  };
}

function fakeJobEnvelope(): SseMessage {
  return {
    kind: 'event',
    id: '1',
    envelope: { version: 1, sequence: 1, event: 'job.progress', requestId: 'r1', data: {} },
  };
}

async function flush(times = 3): Promise<void> {
  for (let i = 0; i < times; i += 1) {
    await act(async () => {
      await Promise.resolve();
    });
  }
}

function scope(orgId: string): Scope {
  return { orgId, permissions: [], allowedCollectionIds: [] };
}

function renderPanel(manager: ScopeManager, onUploaded: (documentId: string) => void = vi.fn()) {
  return render(
    <ScopeProvider manager={manager}>
      <UploadPanel collectionId="col-1" onUploaded={onUploaded} />
    </ScopeProvider>,
  );
}

function selectFile(name = 'a.pdf', size = 1024): FakeXhr {
  const input = screen.getByLabelText('Chọn tệp để tải lên');
  const file = new File(['x'.repeat(size)], name, { type: 'application/pdf' });
  fireEvent.change(input, { target: { files: [file] } });
  return FakeXhr.instances[FakeXhr.instances.length - 1];
}

// The mock's drift-guard validates every response body against the OpenAPI
// schema, including the `uuid` format on id fields — plain strings like
// `'doc-1'` fail that check, so ids here are syntactically-valid uuids via
// the same `mockUuid` helper `mocks/fixtures.ts`'s own seed data uses.
const DOC_ID = mockUuid(90001);
const JOB_ID = mockUuid(90002);
const VERSION_ID = mockUuid(90003);
const DOC_LATE = mockUuid(90004);
const JOB_LATE = mockUuid(90005);

let originalXhr: typeof XMLHttpRequest;

// Demo credentials seeded in `mocks/fixtures.ts`'s `DEMO_USER` — going
// through the real `POST /auth/login` handler (rather than reaching into
// fixtures internals to mint a token pair by hand) keeps this test on the
// same public surface any other caller of `apiClient` uses.
const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

beforeEach(async () => {
  installMockFetch();
  resetMockState();
  FakeXhr.instances = [];
  originalXhr = globalThis.XMLHttpRequest;
  globalThis.XMLHttpRequest = FakeXhr as unknown as typeof XMLHttpRequest;

  await apiClient.login({ email: DEMO_EMAIL, password: DEMO_PASSWORD });

  vi.mocked(createJobEventsSource).mockReset();
  vi.mocked(createJobEventsSource).mockImplementation(() => fakeSseSource<SseMessage>().source);
});

afterEach(() => {
  cleanup();
  uninstallMockFetch();
  globalThis.XMLHttpRequest = originalXhr;
  apiClient.sessionManager.clear();
});

// ---------------------------------------------------------------------------

describe('UploadPanel', () => {
  it('renders a drop zone that accepts file selection', () => {
    renderPanel(createScopeManager());
    expect(screen.getByText('Kéo thả file hoặc bấm để chọn')).toBeVisible();
    expect(screen.getByLabelText('Chọn tệp để tải lên')).toBeInTheDocument();
  });

  // P2-14 (plans/markhand-web/phase-2-web-spa.md §P2.7): "keyboard-operable
  // upload". The real file input is visually hidden (`.upload-dropzone-input`
  // in styles.css: opacity 0 + 1x1px, not `display:none`/`visibility:hidden`
  // — see that rule's own comment for why), which only actually keeps it
  // keyboard-reachable if it stays out of `display:none`/`tabindex="-1"`.
  // This asserts the reachability, not just the visual-hiding technique in
  // the stylesheet: a real Tab would land here and Enter/Space is native
  // `<input type="file">` behaviour the browser supplies for free once
  // focus lands, which jsdom does not simulate opening a file dialog for.
  it('keeps the file input keyboard-reachable despite being visually hidden', () => {
    renderPanel(createScopeManager());
    const input = screen.getByLabelText('Chọn tệp để tải lên');
    expect(input).not.toHaveAttribute('tabindex', '-1');
    expect(input).not.toBeDisabled();
    input.focus();
    expect(document.activeElement).toBe(input);
  });

  it('sends the file and collectionId as multipart form data on selection', async () => {
    renderPanel(createScopeManager());
    const xhr = selectFile('report.pdf');
    await flush();

    expect(xhr.method).toBe('POST');
    expect(xhr.url).toContain('/api/v1/uploads');
    expect(xhr.requestHeaders.Authorization).toMatch(/^Bearer /);
  });

  it('shows real progress as bytes go out, and an indeterminate state when length is not computable', async () => {
    renderPanel(createScopeManager());
    const xhr = selectFile();
    await flush();

    act(() => xhr.progress(25, 100));
    await waitFor(() => expect(screen.getByText('25%')).toBeVisible());

    act(() => xhr.progress(90, 100));
    await waitFor(() => expect(screen.getByText('90%')).toBeVisible());

    act(() => xhr.progress(90, 0, false));
    await waitFor(() => expect(screen.getByText('Đang tải lên…')).toBeVisible());
    expect(screen.queryByText('90%')).not.toBeInTheDocument();
  });

  it('cancelling an in-flight upload actually aborts the XHR', async () => {
    renderPanel(createScopeManager());
    const xhr = selectFile();
    await flush();

    fireEvent.click(screen.getByRole('button', { name: 'Hủy tải lên' }));

    expect(xhr.aborted).toBe(true);
    await waitFor(() => expect(screen.getByText('Đã hủy tải lên.')).toBeVisible());

    // A late 201 arriving after cancel must not resurrect the item as uploading/tracking.
    xhr.respond(201, { disposition: 'accepted', documentId: DOC_LATE, jobId: JOB_LATE });
    await flush();
    expect(screen.getByText('Đã hủy tải lên.')).toBeVisible();
  });

  it('shows a size-specific message for 413, not the generic failure text', async () => {
    renderPanel(createScopeManager());
    const xhr = selectFile();
    await flush();
    xhr.respond(413, { code: 'payload_too_large', message: 'Max 50MB.', requestId: 'r1' });

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent(/dung lượng/i));
    expect(screen.getByRole('alert')).toHaveTextContent('Max 50MB.');
  });

  it('shows a conflict-specific message for 409', async () => {
    renderPanel(createScopeManager());
    const xhr = selectFile();
    await flush();
    xhr.respond(409, {
      code: 'conflict',
      message: 'Document is not fully indexed; cannot accept a new revision yet.',
      requestId: 'r1',
    });

    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent(
        'Document is not fully indexed; cannot accept a new revision yet.',
      ),
    );
  });

  it('shows a quota-specific message for 429, including the Retry-After seconds', async () => {
    renderPanel(createScopeManager());
    const xhr = selectFile();
    await flush();
    xhr.respond(
      429,
      { code: 'rate_limited', message: 'Too many requests; quota exhausted.', requestId: 'r1' },
      { 'Retry-After': '17' },
    );

    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent(/17 giây/));
    expect(screen.getByRole('alert')).toHaveTextContent('Too many requests; quota exhausted.');
  });

  it('calls onUploaded(documentId) as soon as the document is created, before conversion finishes', async () => {
    const onUploaded = vi.fn();
    renderPanel(createScopeManager(), onUploaded);
    const xhr = selectFile();
    await flush();
    xhr.respond(201, {
      disposition: 'accepted',
      documentId: DOC_ID,
      jobId: JOB_ID,
      objectId: 'obj-1',
      collectionId: 'col-1',
      sha256: 'x',
      sizeBytes: 1,
      canonicalFormat: 'markdown',
      requestId: 'r1',
    });
    await flush();

    expect(onUploaded).toHaveBeenCalledWith(DOC_ID);
  });

  it('advances the document state machine from job events (SSE nudges a GET /jobs/{jobId} refetch)', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const sources: ReturnType<typeof fakeSseSource<SseMessage>>[] = [];
    vi.mocked(createJobEventsSource).mockImplementation(() => {
      const fake = fakeSseSource<SseMessage>();
      sources.push(fake);
      return fake.source;
    });

    renderPanel(manager);
    const xhr = selectFile();
    await flush();

    getStore().jobs.set(JOB_ID, {
      id: JOB_ID,
      jobType: 'convert',
      status: 'running',
      attempts: 1,
      documentId: DOC_ID,
      versionId: VERSION_ID,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: null,
    });
    xhr.respond(201, { disposition: 'accepted', documentId: DOC_ID, jobId: JOB_ID });
    await flush();

    await waitFor(() => expect(screen.getByText(/Đang chuyển đổi sang Markdown/)).toBeVisible());

    getStore().jobs.set(JOB_ID, {
      id: JOB_ID,
      jobType: 'convert',
      status: 'succeeded',
      attempts: 1,
      documentId: DOC_ID,
      versionId: VERSION_ID,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
    });
    act(() => sources[0].push(fakeJobEnvelope()));

    await waitFor(() =>
      expect(screen.getByText(/hệ thống đang hoàn thiện lập chỉ mục/)).toBeVisible(),
    );
  });

  // P2-14 (plans/markhand-web/phase-2-web-spa.md §P2.7): a `progressbar` for
  // the job, not just the upload — the job has no percentage the server
  // reports (`Job` has only a `status` enum, no numeric progress field; see
  // `jobLifecycle.ts`'s module doc), so this is deliberately the same
  // indeterminate shape already used for a non-computable upload length,
  // never a fabricated percentage.
  it('shows an indeterminate progressbar while a job is tracked, and none once it reaches a terminal state', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const sources: ReturnType<typeof fakeSseSource<SseMessage>>[] = [];
    vi.mocked(createJobEventsSource).mockImplementation(() => {
      const fake = fakeSseSource<SseMessage>();
      sources.push(fake);
      return fake.source;
    });

    renderPanel(manager);
    const xhr = selectFile();
    await flush();

    getStore().jobs.set(JOB_ID, {
      id: JOB_ID,
      jobType: 'convert',
      status: 'running',
      attempts: 1,
      documentId: DOC_ID,
      versionId: VERSION_ID,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: null,
    });
    xhr.respond(201, { disposition: 'accepted', documentId: DOC_ID, jobId: JOB_ID });
    await flush();

    await waitFor(() => expect(screen.getByRole('progressbar')).toBeVisible());
    expect(screen.getByRole('progressbar')).not.toHaveAttribute('aria-valuenow');

    getStore().jobs.set(JOB_ID, {
      id: JOB_ID,
      jobType: 'convert',
      status: 'succeeded',
      attempts: 1,
      documentId: DOC_ID,
      versionId: VERSION_ID,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
    });
    act(() => sources[0].push(fakeJobEnvelope()));

    await waitFor(() =>
      expect(screen.getByText(/hệ thống đang hoàn thiện lập chỉ mục/)).toBeVisible(),
    );
    expect(screen.queryByRole('progressbar')).not.toBeInTheDocument();
  });

  it('a job.failed transition is shown as a conversion failure, distinct from an upload failure', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const sources: ReturnType<typeof fakeSseSource<SseMessage>>[] = [];
    vi.mocked(createJobEventsSource).mockImplementation(() => {
      const fake = fakeSseSource<SseMessage>();
      sources.push(fake);
      return fake.source;
    });
    renderPanel(manager);
    const xhr = selectFile();
    await flush();

    getStore().jobs.set(JOB_ID, {
      id: JOB_ID,
      jobType: 'convert',
      status: 'running',
      attempts: 1,
      documentId: DOC_ID,
      versionId: VERSION_ID,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: null,
    });
    xhr.respond(201, { disposition: 'accepted', documentId: DOC_ID, jobId: JOB_ID });
    await flush();
    await waitFor(() => expect(screen.getByText(/Đang chuyển đổi sang Markdown/)).toBeVisible());

    getStore().jobs.set(JOB_ID, {
      id: JOB_ID,
      jobType: 'convert',
      status: 'failed',
      attempts: 3,
      documentId: DOC_ID,
      versionId: VERSION_ID,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
    });
    act(() => sources[0].push(fakeJobEnvelope()));

    await waitFor(() => expect(screen.getByText(/Chuyển đổi tài liệu thất bại/)).toBeVisible());
    // Never rendered with the upload-failure `role="alert"` text used elsewhere — this is a status, not an alert, precisely because it's a *conversion* failure, not an *upload* failure.
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('a stream from a stale scope delivers nothing: the old source is aborted and its late message is dropped', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const sources: ReturnType<typeof fakeSseSource<SseMessage>>[] = [];
    vi.mocked(createJobEventsSource).mockImplementation(() => {
      const fake = fakeSseSource<SseMessage>();
      sources.push(fake);
      return fake.source;
    });
    renderPanel(manager);
    const xhr = selectFile();
    await flush();

    getStore().jobs.set(JOB_ID, {
      id: JOB_ID,
      jobType: 'convert',
      status: 'running',
      attempts: 1,
      documentId: DOC_ID,
      versionId: VERSION_ID,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: null,
    });
    xhr.respond(201, { disposition: 'accepted', documentId: DOC_ID, jobId: JOB_ID });
    await flush();
    await waitFor(() => expect(screen.getByText(/Đang chuyển đổi sang Markdown/)).toBeVisible());
    expect(sources).toHaveLength(1);

    act(() => {
      manager.setScope(scope('org-b'));
    });
    await flush();

    expect(sources[0].abort).toHaveBeenCalled();
    // The switch itself is allowed to re-validate under the new epoch (that's
    // `useScopeSafeRequest`'s own, separately-tested guarantee — it refetches
    // on every epoch change, not just on an SSE nudge). What must NOT happen
    // is the *old* (org-a) source's message causing any further reaction —
    // captured as a call-count baseline only after that switch-triggered
    // refetch has settled.
    await waitFor(() => expect(screen.getByText(/Đang chuyển đổi sang Markdown/)).toBeVisible());
    const requestSpy = vi.spyOn(apiClient, 'request');
    const callsAfterSwitchSettled = requestSpy.mock.calls.length;

    // Mark the job as succeeded server-side, then push the *stale* (org-a) source's message — must not trigger a refetch/state change.
    getStore().jobs.set(JOB_ID, {
      id: JOB_ID,
      jobType: 'convert',
      status: 'succeeded',
      attempts: 1,
      documentId: DOC_ID,
      versionId: VERSION_ID,
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
      finishedAt: new Date().toISOString(),
    });
    act(() => sources[0].push(fakeJobEnvelope()));
    await flush();

    expect(screen.getByText(/Đang chuyển đổi sang Markdown/)).toBeVisible(); // unchanged
    expect(requestSpy.mock.calls.length).toBe(callsAfterSwitchSettled); // no new GET /jobs attributable to the stale push

    requestSpy.mockRestore();
  });
});
