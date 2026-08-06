// P2-10 — scope-wide `as_of` mock semantics. Exercised through a real
// `ApiClient` against `installMockFetch()` (same "no hand-stubbed handler"
// convention as `handlers/members.test.ts`), asserting observable ask
// responses rather than private resolver helpers.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../../api/client';
import { installMockFetch, resetMockState, uninstallMockFetch } from '../index';
import {
  getStore,
  ORG_A_ID,
  ORG_B_COLLECTION_ID,
  ORG_B_ID,
  PROJECT_A_HR_ID,
  PROJECT_A_PRODUCT_ID,
  QA_COMPARE_DOCUMENT_ID,
  QA_COMPARE_VERSION_A_ID,
  QA_COMPARE_VERSION_B_ID,
} from '../fixtures';
import { mockTimestamp, mockUuid } from '../ids';

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

async function loggedInClient(): Promise<ApiClient> {
  const client = createApiClient({ baseUrl: '' });
  await client.login({ email: DEMO_EMAIL, password: DEMO_PASSWORD });
  return client;
}

async function switchClientToOrg(client: ApiClient, orgId: string): Promise<void> {
  const tokens = await client.request('post', '/orgs/switch', { body: { orgId } });
  client.sessionManager.setTokens(tokens);
}

function setAllowedCollectionIds(orgId: string, allowedCollectionIds: string[]): void {
  const profile = getStore().orgProfiles.get(orgId);
  if (!profile) throw new Error(`Missing mock org profile for ${orgId}`);
  profile.allowedCollectionIds = allowedCollectionIds;
}

async function expectScopeForbidden(request: Promise<unknown>): Promise<void> {
  try {
    await request;
    throw new Error('Expected Q&A scope request to be forbidden');
  } catch (error) {
    expect(error).toMatchObject({ status: 403, code: 'forbidden' });
    expect(error).not.toHaveProperty('citations');
    expect(error).not.toHaveProperty('versionContext');
  }
}

beforeEach(() => {
  installMockFetch();
  resetMockState();
});

afterEach(() => {
  uninstallMockFetch();
});

describe('ask mock — scope-wide as_of', () => {
  it('rejects missing asOf without falling back to current content', async () => {
    const client = await loggedInClient();
    const body = await client.request('post', '/ask', {
      body: {
        question: 'Ngân sách vận hành là bao nhiêu?',
        mode: 'as_of',
        limit: 10,
      },
    });

    expect(body.citations).toEqual([]);
    expect(body.answer).toMatch(/Không tìm thấy nội dung liên quan/);
    expect((body.warnings ?? []).some((w) => /as-of|asOf|thời điểm/i.test(w))).toBe(true);
    // Must not silently answer from the current (v2 / 15 triệu) budget passage.
    expect(body.answer).not.toMatch(/15 triệu/);
    expect(body.answer).not.toMatch(/10 triệu/);
  });

  it.each([
    ['malformed text', 'not-a-timestamp'],
    ['date only', '2026-01-01'],
    ['numeric-like text', '0'],
    ['missing timezone', '2026-01-01T01:10:00'],
    ['impossible calendar date', '2026-02-30T01:10:00Z'],
  ])('rejects %s asOf without falling back to current content', async (_case, asOf) => {
    const client = await loggedInClient();
    const body = await client.request('post', '/ask', {
      body: {
        question: 'Ngân sách vận hành là bao nhiêu?',
        mode: 'as_of',
        asOf,
        limit: 10,
      },
    });

    expect(body.citations).toEqual([]);
    expect(body.answer).toMatch(/Không tìm thấy nội dung liên quan/);
    expect((body.warnings ?? []).some((w) => /as-of|asOf|thời điểm|hợp lệ/i.test(w))).toBe(true);
    expect(body.answer).not.toMatch(/15 triệu/);
    expect(body.answer).not.toMatch(/10 triệu/);
  });

  it('resolves the latest in-scope effective version without requiring documentId', async () => {
    const client = await loggedInClient();
    // Between budget v1 (mockTimestamp(50)) and v2 (mockTimestamp(95)).
    const asOf = mockTimestamp(70);
    const body = await client.request('post', '/ask', {
      body: {
        question: 'Ngân sách vận hành là bao nhiêu?',
        mode: 'as_of',
        asOf,
        limit: 10,
      },
    });

    expect(body.versionContext?.mode).toBe('as_of');
    expect(body.answer).toMatch(/10 triệu/);
    expect(body.answer).not.toMatch(/15 triệu/);
    expect(body.citations.some((c) => c.versionId === QA_COMPARE_VERSION_A_ID)).toBe(true);
    expect(body.citations.some((c) => c.logicalDocumentId === QA_COMPARE_DOCUMENT_ID)).toBe(true);
    expect(
      (body.warnings ?? []).some(
        (w) => /không phải phiên bản hiện hành/i.test(w) && /10 triệu/.test(w),
      ),
    ).toBe(true);
  });

  it('never cites a passage from a collection outside the resolved project scope', async () => {
    const client = await loggedInClient();
    const asOf = mockTimestamp(70);
    const body = await client.request('post', '/ask', {
      body: {
        question: 'Ngân sách vận hành là bao nhiêu?',
        mode: 'as_of',
        asOf,
        // HR project only owns Employee Handbook — budget doc lives in Product Specs.
        projectIds: [PROJECT_A_HR_ID],
        limit: 10,
      },
    });

    expect(body.citations.every((c) => c.collectionId !== ORG_B_COLLECTION_ID)).toBe(true);
    expect(body.citations.every((c) => c.logicalDocumentId !== QA_COMPARE_DOCUMENT_ID)).toBe(true);
    expect(body.answer).not.toMatch(/10 triệu|15 triệu/);
  });

  it('keeps omitted current and as_of scopes inside the org selected through the switch API', async () => {
    const client = await loggedInClient();
    await switchClientToOrg(client, ORG_B_ID);

    const current = await client.request('post', '/ask', {
      body: {
        question: 'Ngân sách vận hành là bao nhiêu?',
        mode: 'current',
        limit: 10,
      },
    });
    const asOf = await client.request('post', '/ask', {
      body: {
        question: 'Ngân sách vận hành là bao nhiêu?',
        mode: 'as_of',
        asOf: mockTimestamp(100),
        limit: 10,
      },
    });
    const compare = await client.request('post', '/ask', {
      body: {
        question: 'Ngân sách vận hành thay đổi thế nào?',
        mode: 'compare',
        versionA: QA_COMPARE_VERSION_A_ID,
        versionB: QA_COMPARE_VERSION_B_ID,
        limit: 10,
      },
    });
    const history = await client.request('post', '/ask', {
      body: {
        question: 'Lịch sử ngân sách vận hành thế nào?',
        mode: 'history',
        documentId: QA_COMPARE_DOCUMENT_ID,
        limit: 10,
      },
    });

    for (const body of [current, asOf, compare, history]) {
      expect(body.citations.every((c) => c.collectionId === ORG_B_COLLECTION_ID)).toBe(true);
      expect(body.citations.every((c) => c.logicalDocumentId !== QA_COMPARE_DOCUMENT_ID)).toBe(
        true,
      );
      expect(body.answer).not.toMatch(/10 triệu|15 triệu/);
      expect(body.versionContext?.currentVersionIds).not.toContain(QA_COMPARE_VERSION_B_ID);
    }
  });

  it('hard-denies an explicitly requested foreign collection', async () => {
    const client = await loggedInClient();
    await switchClientToOrg(client, ORG_B_ID);

    await expectScopeForbidden(
      client.request('post', '/ask', {
        body: {
          question: 'Ngân sách vận hành là bao nhiêu?',
          mode: 'as_of',
          asOf: mockTimestamp(100),
          collectionIds: [mockUuid(11)],
          limit: 10,
        },
      }),
    );
  });

  it('denies an empty active-profile allow-list on both ask API paths', async () => {
    setAllowedCollectionIds(ORG_A_ID, []);
    const client = await loggedInClient();

    const request = {
      question: 'Ngân sách vận hành là bao nhiêu?',
      mode: 'as_of' as const,
      asOf: mockTimestamp(70),
      limit: 10,
    };
    await expectScopeForbidden(client.request('post', '/ask', { body: request }));
    await expectScopeForbidden(client.request('post', '/ask/stream', { body: request }));
  });

  it('hard-denies same-org disallowed and mixed allowed/foreign collection requests', async () => {
    const allowedCollectionId = mockUuid(10);
    setAllowedCollectionIds(ORG_A_ID, [allowedCollectionId]);
    const client = await loggedInClient();

    await expectScopeForbidden(
      client.request('post', '/ask', {
        body: {
          question: 'Ngân sách vận hành là bao nhiêu?',
          collectionIds: [mockUuid(11)],
          limit: 10,
        },
      }),
    );
    await expectScopeForbidden(
      client.request('post', '/ask/stream', {
        body: {
          question: 'Nhân viên mới cần làm gì?',
          collectionIds: [allowedCollectionId, ORG_B_COLLECTION_ID],
          limit: 10,
        },
      }),
    );
  });

  it('intersects project scope with the active-profile allow-list', async () => {
    // HR contains collection 10, while this profile only allows collection 11.
    setAllowedCollectionIds(ORG_A_ID, [mockUuid(11)]);
    const client = await loggedInClient();

    await expectScopeForbidden(
      client.request('post', '/ask', {
        body: {
          question: 'Nhân viên mới cần làm gì?',
          projectIds: [PROJECT_A_HR_ID],
          limit: 10,
        },
      }),
    );
  });

  it('uses exactly the active-profile allow-list when collection and project scope are omitted', async () => {
    const allowedCollectionId = mockUuid(10);
    setAllowedCollectionIds(ORG_A_ID, [allowedCollectionId]);
    const client = await loggedInClient();

    const body = await client.request('post', '/ask', {
      body: {
        question: 'Nhân viên mới cần hoàn thành gì trong quý này?',
        limit: 10,
      },
    });

    expect(body.citations.length).toBeGreaterThan(0);
    expect(body.citations.every((citation) => citation.collectionId === allowedCollectionId)).toBe(
      true,
    );
    expect(body.versionContext?.currentVersionIds).toEqual([mockUuid(1000)]);
  });

  it('denies a zero-collection project instead of widening its empty resolved scope', async () => {
    const client = await loggedInClient();
    await expectScopeForbidden(
      client.request('post', '/ask', {
        body: {
          question: 'Ngân sách vận hành là bao nhiêu?',
          mode: 'as_of',
          asOf: mockTimestamp(70),
          projectIds: [PROJECT_A_PRODUCT_ID],
          limit: 10,
        },
      }),
    );
  });
});
