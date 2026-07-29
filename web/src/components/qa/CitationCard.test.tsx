// P2-10 gap close: `CitationCard` renders a real Library preview deep-link
// once a `CitationPin` carries `logicalDocumentId`/`collectionId`
// (`hasDeepLink`), and falls back to plain content (no link) when it
// doesn't — see the component's own module doc for the contract history.
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { RouterProvider } from '../../state/RouterProvider';
import { CitationCard, type CitationPin } from './CitationCard';

function basePin(overrides: Partial<CitationPin> = {}): CitationPin {
  return {
    citeId: 'CITE-0001',
    sourceContentSha256: 'a'.repeat(64),
    canonicalMarkdownSha256: 'b'.repeat(64),
    quoteSha256: 'c'.repeat(64),
    chunkIdentitySha256: 'd'.repeat(64),
    quote: 'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
    sourceSpanStart: 0,
    sourceSpanEnd: 10,
    quoteLocalStart: 0,
    quoteLocalEnd: 10,
    isCurrent: true,
    anchor: 'mhcite1.test',
    ...overrides,
  };
}

function renderCard(citation: CitationPin) {
  return render(
    <RouterProvider>
      <ul>
        <CitationCard citation={citation} />
      </ul>
    </RouterProvider>,
  );
}

describe('CitationCard', () => {
  afterEach(() => {
    cleanup();
  });

  it('links to the exact document/version when the pin carries identity', () => {
    const documentId = '11111111-1111-1111-1111-111111111111';
    const collectionId = '22222222-2222-2222-2222-222222222222';
    renderCard(basePin({ logicalDocumentId: documentId, collectionId }));

    const link = screen.getByRole('link', { name: 'Xem trước tài liệu' });
    expect(link).toHaveAttribute('href', `/library/${collectionId}?doc=${documentId}`);
  });

  it('falls back to no link when the pin has no document/collection identity', () => {
    renderCard(basePin());

    expect(screen.queryByRole('link', { name: 'Xem trước tài liệu' })).not.toBeInTheDocument();
  });

  it('still renders quote/citeId/current badge regardless of deep-link availability', () => {
    renderCard(basePin({ isCurrent: false }));

    expect(screen.getByText('CITE-0001')).toBeVisible();
    expect(screen.getByText('Không phải bản hiện hành')).toBeVisible();
    expect(
      screen.getByText('“Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.”'),
    ).toBeVisible();
  });
});
