import { describe, expect, it } from 'vitest';
import { historicalTurnsVisibleWithLive } from './chatLogModel';

describe('historicalTurnsVisibleWithLive', () => {
  it('hides a persisted copy of a question that is still in the live log', () => {
    const visible = historicalTurnsVisibleWithLive(
      [{ question: 'Thông tư 36 2025 nói về nội dung gì' }, { question: 'Còn hiệu lực khi nào?' }],
      new Set(['Thông tư 36 2025 nói về nội dung gì']),
    );
    expect(visible.map((turn) => turn.question)).toEqual(['Còn hiệu lực khi nào?']);
  });

  it('collapses adjacent persisted copies of the same question (double-append)', () => {
    const visible = historicalTurnsVisibleWithLive(
      [
        { question: 'Thông tư 36 2025 nói về nội dung gì' },
        { question: 'Thông tư 36 2025 nói về nội dung gì' },
      ],
      new Set(),
    );
    expect(visible).toHaveLength(1);
  });

  it('keeps distinct adjacent questions from a real conversation', () => {
    const visible = historicalTurnsVisibleWithLive(
      [
        { question: 'Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?' },
        { question: 'Còn ngân sách vận hành thì sao?' },
      ],
      new Set(),
    );
    expect(visible).toHaveLength(2);
  });
});
