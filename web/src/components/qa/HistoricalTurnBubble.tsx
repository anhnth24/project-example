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
import { citationsUsedInAnswer } from './citationFootnoteModel';
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
  const visibleCitations = citationsUsedInAnswer(turn.answer, turn.citations);
  return (
    <div className="chat-turn">
      <div className="chat-turn-question">
        <p className="chat-bubble chat-bubble-user">
          <span className="visually-hidden">Bạn </span>
          {turn.question}
        </p>
      </div>

      <div className="chat-turn-answer">
        <p className="visually-hidden">Trợ lý</p>
        <div className="chat-bubble chat-bubble-assistant">
          <TurnModeBadge answerMode={turn.answerMode} />
          <AnswerText text={turn.answer} citations={visibleCitations} scopeId={scopeId} />
        </div>
      </div>

      <CitationFootnotes
        citations={visibleCitations}
        collectionNameById={collectionNameById}
        scopeId={scopeId}
      />

      <TurnWarningBlocks warnings={turn.warnings} />
    </div>
  );
}
