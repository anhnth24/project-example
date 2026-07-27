// Isolated on purpose: `DocumentRowActions.tsx` destructures its `document`
// prop (the API's `Document` schema, per the fixed component contract) as a
// local binding, which would shadow the DOM's global `document` for the rest
// of that file's scope. Anything that needs the real global lives here,
// where no such prop exists to shadow it.
//
// This is the one place in `components/actions/**` that touches the DOM
// directly to hand fetched bytes to the user. `URL.createObjectURL` here
// always wraps a `Blob` obtained from a response the caller already redeemed
// through the API's own authorization flow (capability issue + single-use
// redeem, see `documentActionsApi.ts`) — it is a local, revocable reference
// to bytes already in memory, not a URL to any storage backend, so it does
// not reintroduce the "guess a storage path" bypass the task calls out.

/**
 * Hands the browser a `Blob` to save, using the classic
 * `URL.createObjectURL` + synthetic `<a download>` click trick. The object
 * URL is revoked shortly after — long enough for the browser to have opened
 * the save stream, short enough not to leak memory if this fires often (bulk
 * downloads from a document list).
 */
export function triggerBrowserDownload(blob: Blob, filename: string): void {
  const objectUrl = URL.createObjectURL(blob);
  const anchor = globalThis.document.createElement('a');
  anchor.href = objectUrl;
  anchor.download = filename;
  anchor.rel = 'noopener';
  anchor.style.display = 'none';
  globalThis.document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(objectUrl), 30_000);
}

/**
 * Derives a `.md` filename for the canonical Markdown download from the
 * document's title, which may itself already carry the original extension
 * (e.g. "Hop dong.docx" — `Document.title` is the original uploaded
 * filename). Strips a single trailing extension, if any, before appending
 * `.md`, so the saved file doesn't come out as "Hop dong.docx.md".
 */
export function markdownFilenameFor(title: string): string {
  const withoutExtension = title.replace(/\.[^./\\]+$/, '');
  const base = withoutExtension.length > 0 ? withoutExtension : title;
  return `${base}.md`;
}
