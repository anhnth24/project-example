// Shared post-answer chrome for live + historical chat turns: mode badge,
// Vietnamese warning summary, optional discarded LLM draft, technical details.
import { Notice } from '../ui';
import { describeAnswerMode } from './answerMode';
import { presentWarnings } from './warningPresentation';

export function TurnModeBadge({ answerMode }: { answerMode?: string }) {
  const modeInfo = describeAnswerMode(answerMode);
  if (!modeInfo) return null;
  if (modeInfo.tone === 'warning') {
    return <Notice tone="warning">{modeInfo.label}</Notice>;
  }
  return (
    <p style={{ margin: 0 }}>
      <span className="tag tag-neutral">{modeInfo.label}</span>
    </p>
  );
}

export function TurnWarningBlocks({ warnings }: { warnings: readonly string[] }) {
  const { summary, technicalDetails, discardedLlmDraft } = presentWarnings(warnings);
  if (!summary && !discardedLlmDraft && technicalDetails.length === 0) return null;

  return (
    <div className="chat-turn-meta">
      {summary && <Notice tone="warning">{summary}</Notice>}

      {discardedLlmDraft && (
        <details data-testid="qa-discarded-llm-draft">
          <summary>Bản nháp mô hình (không đạt kiểm chứng)</summary>
          <p style={{ whiteSpace: 'pre-wrap', margin: 'var(--space-2) 0 0', lineHeight: 1.55 }}>
            {discardedLlmDraft}
          </p>
        </details>
      )}

      {technicalDetails.length > 0 && (
        <details data-testid="qa-warning-details">
          <summary>Chi tiết kỹ thuật ({technicalDetails.length})</summary>
          <ul style={{ margin: 'var(--space-2) 0 0', paddingLeft: '1.25rem' }}>
            {technicalDetails.map((detail, i) => (
              <li key={i} className="text-muted" style={{ marginBottom: 'var(--space-1)' }}>
                {detail}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}
