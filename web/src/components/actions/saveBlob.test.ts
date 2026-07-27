import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { markdownFilenameFor, triggerBrowserDownload } from './saveBlob';

describe('markdownFilenameFor', () => {
  it('replaces a single trailing extension with .md', () => {
    expect(markdownFilenameFor('Hop dong dich vu.docx')).toBe('Hop dong dich vu.md');
    expect(markdownFilenameFor('Roadmap.xlsx')).toBe('Roadmap.md');
  });

  it('appends .md when the title has no extension', () => {
    expect(markdownFilenameFor('README')).toBe('README.md');
  });

  it('only strips the final extension, not dots earlier in the name', () => {
    expect(markdownFilenameFor('v2.1 Report.final.pdf')).toBe('v2.1 Report.final.md');
  });
});

describe('triggerBrowserDownload', () => {
  // jsdom (the vitest "environment: jsdom" this project uses) does not
  // implement `URL.createObjectURL`/`revokeObjectURL` at all — confirmed by
  // running it directly (`typeof URL.createObjectURL === 'undefined'`), not
  // assumed — so both are stubbed here rather than exercised for real.
  let createObjectURL: ReturnType<typeof vi.fn>;
  let revokeObjectURL: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    createObjectURL = vi.fn(() => 'blob:mock-url');
    revokeObjectURL = vi.fn();
    vi.stubGlobal('URL', { ...URL, createObjectURL, revokeObjectURL });
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('creates a temporary anchor, clicks it with the given filename, then removes it', () => {
    const blob = new Blob(['hello'], { type: 'text/markdown' });
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});

    triggerBrowserDownload(blob, 'Hop dong.md');

    expect(createObjectURL).toHaveBeenCalledWith(blob);
    expect(clickSpy).toHaveBeenCalledOnce();
    // The anchor must not still be attached to the document after the click.
    expect(document.querySelectorAll('a[download="Hop dong.md"]')).toHaveLength(0);
    clickSpy.mockRestore();
  });

  it('revokes the object URL after a delay instead of immediately', () => {
    const blob = new Blob(['hello']);
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});

    triggerBrowserDownload(blob, 'file.md');
    expect(revokeObjectURL).not.toHaveBeenCalled();

    vi.advanceTimersByTime(30_000);
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:mock-url');
  });
});
