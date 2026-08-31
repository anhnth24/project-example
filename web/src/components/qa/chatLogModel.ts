// Pure helpers for the chat log: after a live turn persists, `getChatSession`
// reloads the same question into `historicalTurns` while the live bubble is
// still mounted. Adjacent persisted copies of one ask (double-append) must
// also collapse. Kept out of `ChatPanel.tsx` so the rules can be unit-tested
// without mounting the composer.

export function historicalTurnsVisibleWithLive<T extends { question: string }>(
  historicalTurns: readonly T[],
  liveQuestions: ReadonlySet<string>,
): T[] {
  const visible: T[] = [];
  for (const turn of historicalTurns) {
    if (liveQuestions.has(turn.question)) continue;
    const previous = visible[visible.length - 1];
    if (previous !== undefined && previous.question === turn.question) continue;
    visible.push(turn);
  }
  return visible;
}
