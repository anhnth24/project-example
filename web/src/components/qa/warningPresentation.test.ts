import { describe, expect, it } from 'vitest';
import { DISCARDED_LLM_DRAFT_PREFIX, presentWarnings } from './warningPresentation';

describe('presentWarnings', () => {
  it('summarizes grounding failures in Vietnamese and keeps English lines in details', () => {
    const presented = presentWarnings([
      'Factual claim lacks citation; unverifiable: CASAN là khung điều phối.',
      'Unverifiable claim-level grounding; using extractive fallback.',
    ]);
    expect(presented.summary).toContain('không đạt kiểm chứng');
    expect(presented.technicalDetails).toHaveLength(2);
    expect(presented.discardedLlmDraft).toBeNull();
  });

  it('extracts the UAT discarded LLM draft prefix and does not show it as a technical line', () => {
    const draft = 'CASAN gồm năm bước… [CITE-0001]';
    const presented = presentWarnings([
      `${DISCARDED_LLM_DRAFT_PREFIX}${draft}`,
      'Unverifiable claim-level grounding; using extractive fallback.',
    ]);
    expect(presented.discardedLlmDraft).toBe(draft);
    expect(
      presented.technicalDetails.every((line) => !line.startsWith(DISCARDED_LLM_DRAFT_PREFIX)),
    ).toBe(true);
    expect(presented.summary).toContain('không đạt kiểm chứng');
  });

  it('returns null summary when there are no warnings', () => {
    expect(presentWarnings([])).toEqual({
      summary: null,
      technicalDetails: [],
      discardedLlmDraft: null,
    });
  });
});
