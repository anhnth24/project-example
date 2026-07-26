import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { SafeMarkdown } from './SafeMarkdown';
import { MAX_MARKDOWN_LENGTH } from '../lib/sanitize';

describe('SafeMarkdown', () => {
  afterEach(() => {
    cleanup();
  });

  it('renders trusted Markdown content', () => {
    const { container } = render(
      <SafeMarkdown>
        {'# Tiêu đề\n\nĐoạn văn **in đậm** và [liên kết](https://example.com).'}
      </SafeMarkdown>,
    );

    expect(screen.getByRole('heading', { name: 'Tiêu đề' })).toBeVisible();
    const link = container.querySelector('a');
    expect(link?.getAttribute('href')).toBe('https://example.com');
  });

  describe('raw HTML', () => {
    it('strips <script> tags', () => {
      const { container } = render(
        <SafeMarkdown>{'Xin chào\n\n<script>window.__xss = "script"</script>'}</SafeMarkdown>,
      );

      expect(container.querySelector('script')).toBeNull();
      expect(container.innerHTML).not.toContain('<script');
    });

    it('strips inline event-handler attributes', () => {
      // Note: react-markdown builds real React elements (no dangerouslySetInnerHTML),
      // so React itself already refuses a *string* value for an `onError`/`onClick`-style
      // prop (it only accepts a function). This assertion documents that the attribute
      // never reaches the DOM either way; it is not, by itself, evidence that
      // rehype-sanitize is doing the blocking here (see the mutation-test note below and
      // the `style` case right after, which *is* sanitizer-dependent).
      const { container } = render(
        <SafeMarkdown>
          {'<img src="x" alt="test" onerror="window.__xss = \'onerror\'">'}
        </SafeMarkdown>,
      );

      const img = container.querySelector('img');
      expect(img).not.toBeNull();
      expect(img?.getAttribute('onerror')).toBeNull();
      expect(container.innerHTML).not.toContain('onerror');
    });

    it('strips style attributes (CSS-based attribute injection)', () => {
      // Unlike on*-handler props, React *does* apply a style object built from a raw
      // `style="..."` string, so this one only goes away because rehype-sanitize's
      // schema does not allow `style` on any tag. Real mutation-test coverage for the
      // "event handler / dangerous attribute" vector.
      const { container } = render(
        <SafeMarkdown>
          {'<p style="background-image: url(https://evil.example/track.gif)">hi</p>'}
        </SafeMarkdown>,
      );

      const p = container.querySelector('p');
      expect(p?.getAttribute('style')).toBeNull();
      expect(container.innerHTML).not.toContain('background-image');
    });

    it('strips <iframe> elements', () => {
      const { container } = render(
        <SafeMarkdown>{'<iframe src="https://evil.example"></iframe>'}</SafeMarkdown>,
      );

      expect(container.querySelector('iframe')).toBeNull();
    });

    it('strips <object> elements', () => {
      const { container } = render(
        <SafeMarkdown>
          {'<object data="https://evil.example" type="text/html"></object>'}
        </SafeMarkdown>,
      );

      expect(container.querySelector('object')).toBeNull();
    });
  });

  describe('dangerous link protocols', () => {
    it.each([
      ['javascript:', '[click](javascript:window.__xss=1)'],
      ['JavaScript: (case obfuscation)', '[click](JavaScript:window.__xss=1)'],
      [
        'leading-whitespace javascript: (whitespace obfuscation)',
        '<a href="  javascript:window.__xss=1">click</a>',
      ],
      ['tab-obfuscated javascript:', '<a href="java&#9;script:window.__xss=1">click</a>'],
      ['vbscript:', '<a href="vbscript:msgbox(1)">click</a>'],
      ['data:text/html', '[click](data:text/html,<script>window.__xss=1</script>)'],
    ])('removes href for %s', (_label, markdown) => {
      const { container } = render(<SafeMarkdown>{markdown}</SafeMarkdown>);

      const link = container.querySelector('a');
      expect(link).not.toBeNull();
      expect(link?.hasAttribute('href')).toBe(false);
    });

    it('keeps safe http(s)/mailto links', () => {
      const { container } = render(
        <SafeMarkdown>{'[a](https://example.com) [b](mailto:test@example.com)'}</SafeMarkdown>,
      );

      const links = container.querySelectorAll('a');
      expect(links).toHaveLength(2);
      expect(links[0].getAttribute('href')).toBe('https://example.com');
      expect(links[1].getAttribute('href')).toBe('mailto:test@example.com');
    });
  });

  describe('SVG and data: URLs in images', () => {
    it('strips inline <svg> raw HTML, including onload/embedded <script>', () => {
      const { container } = render(
        <SafeMarkdown>
          {'<svg onload="window.__xss=1"><script>window.__xss2=1</script></svg>'}
        </SafeMarkdown>,
      );

      expect(container.querySelector('svg')).toBeNull();
      expect(container.querySelector('script')).toBeNull();
      expect(container.innerHTML).not.toContain('onload');
    });

    it('strips data: URLs from Markdown image src (including SVG data URLs)', () => {
      const { container } = render(
        <SafeMarkdown>
          {'![alt](data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9ImFsZXJ0KDEpIi8+)'}
        </SafeMarkdown>,
      );

      const img = container.querySelector('img');
      expect(img).not.toBeNull();
      expect(img?.hasAttribute('src')).toBe(false);
    });

    it('keeps http(s) image src', () => {
      const { container } = render(
        <SafeMarkdown>{'![alt](https://example.com/a.png)'}</SafeMarkdown>,
      );

      const img = container.querySelector('img');
      expect(img?.getAttribute('src')).toBe('https://example.com/a.png');
    });
  });

  describe('oversized content', () => {
    it('clamps Markdown longer than MAX_MARKDOWN_LENGTH and shows a notice', () => {
      const oversized = '#'.repeat(MAX_MARKDOWN_LENGTH + 10_000);

      const { container } = render(<SafeMarkdown>{oversized}</SafeMarkdown>);

      expect(container.textContent?.length ?? 0).toBeLessThan(oversized.length);
      expect(screen.getByRole('note')).toHaveTextContent('rút gọn');
    });

    it('does not clamp content within the bound and shows no notice', () => {
      const withinBound = 'nội dung bình thường';

      render(<SafeMarkdown>{withinBound}</SafeMarkdown>);

      expect(screen.queryByRole('note')).toBeNull();
    });
  });

  describe('DOM clobbering', () => {
    it('prefixes clobber-prone id/name attributes instead of leaving them raw', () => {
      const { container } = render(
        <SafeMarkdown>{'<img name="parentNode" src="https://example.com/a.png">'}</SafeMarkdown>,
      );

      const img = container.querySelector('img');
      expect(img?.getAttribute('name')).not.toBe('parentNode');
      expect(img?.getAttribute('name')).toMatch(/^user-content-/);
    });
  });
});
