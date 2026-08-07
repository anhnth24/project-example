import { describe, expect, it } from 'vitest';
import { createPlaywrightConfig } from '../../playwright.config';

describe('createPlaywrightConfig', () => {
  it('real mode runs serially with browser artifacts disabled', () => {
    const config = createPlaywrightConfig({ MARKHAND_E2E_REAL: '1' });

    expect(config.workers).toBe(1);
    expect(config.fullyParallel).toBe(false);
    expect(config.projects?.length).toBe(1);
    expect(config.projects?.[0]?.name).toBe('real');
    expect(config.projects?.[0]?.testDir).toBe('./e2e-real');
    expect(config.use?.trace).toBe('off');
    expect(config.use?.screenshot).toBe('off');
    expect(config.use?.video).toBe('off');
    expect(config.webServer).toBeUndefined();
  });

  it('mock mode retains parallel execution and on-first-retry trace', () => {
    const config = createPlaywrightConfig({});

    expect(config.fullyParallel).toBe(true);
    expect(config.workers).not.toBe(1);
    expect(config.projects?.length).toBe(1);
    expect(config.projects?.[0]?.name).toBe('chromium');
    expect(config.projects?.[0]?.testDir).toBeUndefined();
    expect(config.use?.trace).toBe('on-first-retry');
    expect(config.use?.screenshot).not.toBe('off');
    expect(config.use?.video).not.toBe('off');
    expect(config.webServer).toBeDefined();
  });
});
