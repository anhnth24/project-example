// Contract tests for the Playwright config factory: real mode must be serial
// with browser artifacts disabled; mock mode must retain current parallel +
// on-first-retry trace behavior. Projects remain mutually exclusive.
import { describe, expect, it } from 'vitest';
import { createPlaywrightConfig } from '../../playwright.config';

describe('createPlaywrightConfig', () => {
  it('real mode runs one worker, disables parallelism, and turns off artifacts', () => {
    const config = createPlaywrightConfig({ MARKHAND_E2E_REAL: '1' });

    expect(config.workers).toBe(1);
    expect(config.fullyParallel).toBe(false);
    expect(config.use?.trace).toBe('off');
    expect(config.use?.screenshot).toBe('off');
    expect(config.use?.video).toBe('off');

    const projects = config.projects ?? [];
    expect(projects).toHaveLength(1);
    expect(projects[0]?.name).toBe('real');
    expect(projects[0]?.testDir).toMatch(/e2e-real$/);
    expect(projects.some((project) => project.name === 'chromium')).toBe(false);
    expect(config.webServer).toBeUndefined();
  });

  it('mock mode retains parallel workers and on-first-retry traces', () => {
    const config = createPlaywrightConfig({});

    expect(config.fullyParallel).toBe(true);
    expect(config.workers).toBeUndefined();
    expect(config.use?.trace).toBe('on-first-retry');
    // Mock retains prior behavior: screenshot/video are not forced off here.
    expect(config.use?.screenshot).toBeUndefined();
    expect(config.use?.video).toBeUndefined();

    const projects = config.projects ?? [];
    expect(projects).toHaveLength(1);
    expect(projects[0]?.name).toBe('chromium');
    expect(projects.some((project) => project.name === 'real')).toBe(false);
    expect(config.webServer).toBeDefined();
  });
});
