// Honest rendering of the document state machine from plan P2.4
// ("uploaded|converting|converted|indexing|indexed|failed shown honestly").
// Reuses the app's existing `.tag*` classes and `SpinnerIcon` — no new CSS.
import { SpinnerIcon } from '../icons';
import { DOCUMENT_STATE_META } from './documentPresentation';
import type { DocumentState } from './types';

export function DocumentStateBadge({ state }: { state: DocumentState }) {
  const meta = DOCUMENT_STATE_META[state];
  return (
    <span className={`tag ${meta.tagClass}`}>
      {meta.spinning && (
        <SpinnerIcon
          className="spin"
          size={11}
          style={{ marginRight: 'var(--space-1)' }}
          aria-hidden="true"
        />
      )}
      {meta.label}
    </span>
  );
}
