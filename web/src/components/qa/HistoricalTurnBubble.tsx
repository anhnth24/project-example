// Renders one already-stored `ChatTurn` (from `getChatSession`, P2-19) with
// the exact same visual shape a live `ChatTurnBubble` settles into — question
// row, footnoted answer, mode badge, warnings, numbered citation sources —
// per the task brief's "mỗi turn đủ answer/mode-badge/warnings/citations như
// bubble live". Deliberately has no `useAskStream`/streaming state at all:
// a historical turn arrived fully formed from the server, there is nothing
// left to stream.
import type { components } from '../../api/generated/contract';
import { AnswerText } from './AnswerText';
import { CitationFootnotes } from './CitationFootnotes';
import { TurnModeBadge, TurnWarningBlocks } from './TurnAnswerMeta';

type ChatTurn = components['schemas']['ChatTurn'];

export function HistoricalTurnBubble({
  turn,
  collectionNameById,
}: {
  turn: ChatTurn;
  collectionNameById: ReadonlyMap<string, string>;
}) {
  const scopeId = `history-${turn.id}`;
  return (
    <div className="chat-turn" style={{ display: 'grid', gap: 'var(--space-3)' }}>
      <p style={{ margin: 0 }}>
        <span className="tag tag-outline">Bạn</span>{' '}
        <span style={{ fontWeight: 600 }}>{turn.question}</span>
      </p>

      <div style={{ display: 'grid', gap: 'var(--space-2)' }}>
        <p style={{ margin: 0 }}>
          <span className="tag tag-outline">Trợ lý</span>
        </p>
        <TurnModeBadge answerMode={turn.answerMode} />
        <AnswerText text={turn.answer} citations={turn.citations} scopeId={scopeId} />
      </div>

      <CitationFootnotes
        citations={turn.citations}
        collectionNameById={collectionNameById}
        scopeId={scopeId}
      />

      <TurnWarningBlocks warnings={turn.warnings} />
    </div>
  );
}
