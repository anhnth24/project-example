// P2-14 (plans/markhand-web/phase-2-web-spa.md §P2.7): "axe không có
// critical violations". This is the one place axe-core actually runs
// against this app's real rendered states — the login page, the library
// page with documents and a selected document, and an open modal — rather
// than against isolated markup fixtures. `axe-core` is a devDependency only
// (see package.json); there is no Playwright in this repo and this task
// does not add one, so this suite is Vitest + Testing Library like every
// other test here.
//
// Contrast is explicitly out of scope for this file: jsdom has no layout
// engine and does not resolve real painted colours (no CSSOM box model, no
// browser compositing), so axe's `color-contrast` rule cannot report
// anything meaningful under jsdom — it is disabled at every call below,
// with this same reasoning, rather than silenced globally. Contrast in this
// project is verified by hand and recorded as `contrast:` comments in
// styles.css (see that file's top-of-file comment for the convention).
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import axe, { type RunOptions } from 'axe-core';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from './api/client';
import type { components } from './api/generated/contract';
import { installMockFetch, resetMockState, uninstallMockFetch } from './mocks';
import { getStore, nextId } from './mocks/fixtures';
import { mockTimestamp } from './mocks/ids';
import { LibraryPage } from './pages/LibraryPage';
import { LoginPage } from './pages/LoginPage';
import { RouterProvider } from './state/RouterProvider';
import { ScopeProvider } from './state/ScopeProvider';

type Collection = components['schemas']['Collection'];
type LibraryDocument = components['schemas']['Document'];
type DocumentVersion = components['schemas']['DocumentVersion'];

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

// `color-contrast` disabled everywhere in this file: see the module doc
// above — jsdom cannot compute real painted colour, so this rule can only
// ever produce false positives/negatives here, never a real signal.
const AXE_OPTIONS: RunOptions = {
  rules: { 'color-contrast': { enabled: false } },
};

function seedCollection(overrides: Partial<Collection> = {}): Collection {
  const collection: Collection = {
    id: nextId(),
    name: 'Bộ sưu tập kiểm thử',
    slug: 'test-collection',
    description: null,
    visibility: 'org',
    createdAt: mockTimestamp(0),
    ...overrides,
  };
  getStore().collections.push(collection);
  return collection;
}

/** A document that already has a current version, so the preview panel actually renders content. */
function seedDocumentWithVersion(
  collectionId: string,
  overrides: Partial<LibraryDocument> = {},
): LibraryDocument {
  const versionId = nextId();
  const doc: LibraryDocument = {
    id: nextId(),
    collectionId,
    title: 'Tài liệu.pdf',
    state: 'indexed',
    currentVersionId: versionId,
    createdAt: mockTimestamp(0),
    updatedAt: mockTimestamp(0),
    ...overrides,
  };
  const docs = getStore().documents.get(collectionId) ?? [];
  docs.push(doc);
  getStore().documents.set(collectionId, docs);
  const version: DocumentVersion = {
    id: versionId,
    documentId: doc.id,
    versionNumber: 1,
    isCurrent: true,
    sourceContentSha256: 'a'.repeat(64),
    effectiveFrom: mockTimestamp(0),
    effectiveTo: null,
    changeSummary: null,
    createdAt: mockTimestamp(0),
  };
  getStore().versions.set(doc.id, [version]);
  return doc;
}

async function loggedInClient(): Promise<ApiClient> {
  const client = createApiClient({ baseUrl: '' });
  await client.login({ email: DEMO_EMAIL, password: DEMO_PASSWORD });
  return client;
}

/**
 * Fails with every `critical`/`serious` violation's rule id and target
 * selectors, so a real failure here is diagnosable from the test output
 * alone rather than requiring a re-run with extra logging.
 */
async function expectNoSeriousViolations(container: Element): Promise<void> {
  const results = await axe.run(container, AXE_OPTIONS);
  const bad = results.violations.filter(
    (violation) => violation.impact === 'critical' || violation.impact === 'serious',
  );
  if (bad.length > 0) {
    const detail = bad
      .map(
        (violation) =>
          `${violation.id} (${violation.impact}): ${violation.nodes
            .map((node) => node.target.join(' '))
            .join(', ')}`,
      )
      .join('\n');
    throw new Error(`axe found ${bad.length} critical/serious violation(s):\n${detail}`);
  }
}

describe('accessibility (axe-core, no critical/serious violations)', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
  });

  afterEach(() => {
    cleanup();
    uninstallMockFetch();
  });

  it('the login page', async () => {
    const { container } = render(<LoginPage />);
    await expectNoSeriousViolations(container);
  });

  it('the library page with documents and a selected document', async () => {
    const collection = seedCollection();
    seedDocumentWithVersion(collection.id, { title: 'Báo cáo tài chính.pdf' });
    seedDocumentWithVersion(collection.id, { title: 'Hợp đồng.docx', state: 'converting' });
    const client = await loggedInClient();

    const { container } = render(
      <RouterProvider>
        <ScopeProvider>
          <LibraryPage collectionId={collection.id} client={client} />
        </ScopeProvider>
      </RouterProvider>,
    );

    fireEvent.click(await screen.findByRole('button', { name: /Báo cáo tài chính\.pdf/ }));
    await waitFor(() =>
      expect(
        screen.getByRole('heading', { level: 2, name: 'Báo cáo tài chính.pdf' }),
      ).toBeVisible(),
    );

    await expectNoSeriousViolations(container);
  });

  it('an open modal (delete confirmation)', async () => {
    const collection = seedCollection();
    seedDocumentWithVersion(collection.id, { title: 'Sẽ bị xóa.pdf' });
    const client = await loggedInClient();

    const { container } = render(
      <RouterProvider>
        <ScopeProvider>
          <LibraryPage collectionId={collection.id} client={client} />
        </ScopeProvider>
      </RouterProvider>,
    );

    fireEvent.click(await screen.findByRole('button', { name: /Sẽ bị xóa\.pdf/ }));
    fireEvent.click(await screen.findByRole('button', { name: 'Xóa' }));
    await waitFor(() => expect(screen.getByRole('dialog')).toBeVisible());

    await expectNoSeriousViolations(container);
  });
});
