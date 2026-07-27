import { describe, expect, it } from 'vitest';
import { describeJobPhase, isTerminalPhase } from './jobLifecycle';
import type { Job } from './types';

describe('describeJobPhase', () => {
  it.each<[Job['status'], ReturnType<typeof describeJobPhase>]>([
    ['pending', 'converting'],
    ['leased', 'converting'],
    ['running', 'converting'],
    ['succeeded', 'converted'],
    ['failed', 'failed'],
    ['cancelled', 'failed'],
    ['dead_letter', 'failed'],
  ])('maps job status %s to %s', (status, expected) => {
    expect(describeJobPhase(status)).toBe(expected);
  });
});

describe('isTerminalPhase', () => {
  it('is terminal for converted and failed only', () => {
    expect(isTerminalPhase('converting')).toBe(false);
    expect(isTerminalPhase('converted')).toBe(true);
    expect(isTerminalPhase('failed')).toBe(true);
  });
});
