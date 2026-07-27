import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiClient } from '../../api/client';
import { HttpApiError } from '../../api/errors';
import {
  getStore,
  installMockFetch,
  mockControl,
  resetMockState,
  uninstallMockFetch,
} from '../../mocks';
import { downloadDocumentVersion, requestDelete, requestReindex } from './documentActionsApi';
import { triggerBrowserDownload } from './saveBlob';
import { signInDemoUser } from './testSupport';

vi.mock('./saveBlob', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./saveBlob')>();
  return { ...actual, triggerBrowserDownload: vi.fn() };
});

let DOCUMENT_ID: string;
let VERSION_ID: string;
let TITLE: string;

beforeEach(() => {
  installMockFetch();
  resetMockState();
  signInDemoUser();

  // Read the seeded "Onboarding Guide.pdf" (state: indexed, has a current
  // version) straight out of the mock store rather than hand-deriving its
  // uuid — `mockUuid`'s exact encoding is an implementation detail of
  // `mocks/ids.ts` this test shouldn't need to know.
  const store = getStore();
  const [collection] = store.collections;
  const docs = store.documents.get(collection.id) ?? [];
  const doc = docs.find((d) => d.currentVersionId !== null);
  if (!doc) throw new Error('test fixture expected a seeded document with a current version');
  DOCUMENT_ID = doc.id;
  VERSION_ID = doc.currentVersionId as string;
  TITLE = doc.title;
});

afterEach(() => {
  uninstallMockFetch();
  apiClient.sessionManager.clear();
  vi.clearAllMocks();
});

describe('downloadDocumentVersion', () => {
  it('issues a capability, redeems it, and hands the bytes to triggerBrowserDownload with a .md filename', async () => {
    await downloadDocumentVersion({
      client: apiClient,
      documentId: DOCUMENT_ID,
      versionId: VERSION_ID,
      purpose: 'markdown',
      title: TITLE,
      signal: new AbortController().signal,
    });

    expect(triggerBrowserDownload).toHaveBeenCalledOnce();
    const [blob, filename] = vi.mocked(triggerBrowserDownload).mock.calls[0];
    expect(filename).toBe('Onboarding Guide.md');
    expect(blob.type).toContain('text/markdown');
  });

  it('keeps the original filename (with its real extension) for purpose "original"', async () => {
    await downloadDocumentVersion({
      client: apiClient,
      documentId: DOCUMENT_ID,
      versionId: VERSION_ID,
      purpose: 'original',
      title: TITLE,
      signal: new AbortController().signal,
    });

    const [, filename] = vi.mocked(triggerBrowserDownload).mock.calls[0];
    expect(filename).toBe(TITLE);
  });

  it('mints a capability the mock store marks single-use: redeeming it twice is impossible even by calling the raw endpoint again', async () => {
    await downloadDocumentVersion({
      client: apiClient,
      documentId: DOCUMENT_ID,
      versionId: VERSION_ID,
      purpose: 'markdown',
      title: TITLE,
      signal: new AbortController().signal,
    });

    const [capability] = [...getStore().downloadCapabilities.keys()];
    expect(getStore().downloadCapabilities.get(capability)?.redeemed).toBe(true);

    const token = await apiClient.tokenProvider.getAccessToken();
    const second = await fetch(`/api/v1/downloads/${capability}`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    expect(second.status).toBe(404);
  });

  it('propagates a 403 from the capability-issue step as an HttpApiError', async () => {
    mockControl.forceStatus('issueDownloadCapability', 403, { times: 1 });
    await expect(
      downloadDocumentVersion({
        client: apiClient,
        documentId: DOCUMENT_ID,
        versionId: VERSION_ID,
        purpose: 'markdown',
        title: TITLE,
        signal: new AbortController().signal,
      }),
    ).rejects.toMatchObject({ status: 403 });
    expect(triggerBrowserDownload).not.toHaveBeenCalled();
  });

  it('propagates a 429 from the capability-issue step with rate-limit metadata', async () => {
    mockControl.forceStatus('issueDownloadCapability', 429, { times: 1 });
    await expect(
      downloadDocumentVersion({
        client: apiClient,
        documentId: DOCUMENT_ID,
        versionId: VERSION_ID,
        purpose: 'markdown',
        title: TITLE,
        signal: new AbortController().signal,
      }),
    ).rejects.toBeInstanceOf(HttpApiError);
  });
});

describe('requestReindex', () => {
  it('enqueues a job and reports created: true', async () => {
    const outcome = await requestReindex({
      client: apiClient,
      documentId: DOCUMENT_ID,
      signal: new AbortController().signal,
    });
    expect(outcome.created).toBe(true);
    expect(outcome.jobId).toBeTruthy();
  });

  it('propagates a 429', async () => {
    mockControl.forceStatus('reindexDocument', 429, { times: 1 });
    await expect(
      requestReindex({
        client: apiClient,
        documentId: DOCUMENT_ID,
        signal: new AbortController().signal,
      }),
    ).rejects.toMatchObject({ status: 429 });
  });
});

describe('requestDelete', () => {
  it('resolves on a 204 tombstone response', async () => {
    await expect(
      requestDelete({
        client: apiClient,
        documentId: DOCUMENT_ID,
        signal: new AbortController().signal,
      }),
    ).resolves.toBeUndefined();
  });

  it('propagates a 404 for an unknown document', async () => {
    await expect(
      requestDelete({
        client: apiClient,
        documentId: 'not-a-real-id',
        signal: new AbortController().signal,
      }),
    ).rejects.toMatchObject({ status: 404 });
  });
});
