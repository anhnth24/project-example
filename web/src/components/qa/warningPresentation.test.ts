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

  it('hides fail-closed extractive-only noise — the mode badge already says "đoạn nguồn"', () => {
    expect(
      presentWarnings([
        'Structured entailment unavailable; fail-closed extractive-only grounding.',
      ]),
    ).toEqual({
      summary: null,
      technicalDetails: [],
      discardedLlmDraft: null,
    });
  });

  it('does not describe an embedding timeout as a chat-model timeout', () => {
    const presented = presentWarnings([
      'Embedding timed out; using FTS-only retrieval.',
      'Structured entailment unavailable; fail-closed extractive-only grounding.',
    ]);
    expect(presented.summary).toContain('Tìm kiếm theo nghĩa chưa kịp');
    expect(presented.summary).not.toContain('Nhà cung cấp mô hình hết thời gian chờ');
    expect(
      presented.technicalDetails.some((line) => /embedding hết thời gian chờ/i.test(line)),
    ).toBe(true);
  });

  it('renders known server warnings in Vietnamese in technical details', () => {
    const presented = presentWarnings([
      'Removed 1 unverifiable sentence(s) from LLM draft; remainder passed claim checks.',
      'Dev-gate: LLM answer passed citation/claim checks but structured entailment is NOT available — this answer is unverified, not grounded.',
    ]);
    expect(presented.technicalDetails).toEqual([
      'Đã loại 1 câu không kiểm chứng được khỏi bản nháp của mô hình; phần còn lại đã vượt qua kiểm tra trích dẫn.',
      'Chế độ thử nghiệm: câu trả lời của mô hình đã qua kiểm tra trích dẫn, nhưng bộ kiểm chứng suy diễn (structured entailment) chưa sẵn sàng — câu trả lời này chưa được xác minh đầy đủ.',
    ]);
  });

  it('translates the embedding-retry-recovered warning (P0.0)', () => {
    const presented = presentWarnings([
      'Embedding recovered after one retry (transient provider error).',
    ]);
    expect(
      [presented.summary, ...presented.technicalDetails].includes(
        'Dịch vụ embedding phục hồi sau một lần thử lại (lỗi thoáng qua).',
      ),
    ).toBe(true);
  });

  it('translates the citation-auto-attach warning with count (P0.2)', () => {
    const presented = presentWarnings([
      'Auto-attached citations to 2 sentence(s); full draft passed claim checks.',
    ]);
    expect(
      [presented.summary, ...presented.technicalDetails].includes(
        'Hệ thống đã tự gắn trích dẫn cho 2 câu; toàn bộ câu trả lời đã vượt qua kiểm tra trích dẫn.',
      ),
    ).toBe(true);
  });

  it('passes unknown server warnings through untranslated (as the summary)', () => {
    const presented = presentWarnings(['Some brand-new server warning nobody mapped yet.']);
    expect(presented.summary).toBe('Some brand-new server warning nobody mapped yet.');
  });

  it('returns null summary when there are no warnings', () => {
    expect(presentWarnings([])).toEqual({
      summary: null,
      technicalDetails: [],
      discardedLlmDraft: null,
    });
  });
});
