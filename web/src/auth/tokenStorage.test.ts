import { afterEach, describe, expect, it } from 'vitest';
import {
  clearPersistedRefreshToken,
  loadPersistedRefreshToken,
  savePersistedRefreshToken,
} from './tokenStorage';

afterEach(() => {
  window.sessionStorage.clear();
});

describe('tokenStorage', () => {
  it('returns null when nothing has been persisted', () => {
    expect(loadPersistedRefreshToken()).toBeNull();
  });

  it('round-trips a saved refresh token', () => {
    savePersistedRefreshToken('refresh-abc');
    expect(loadPersistedRefreshToken()).toBe('refresh-abc');
  });

  it('overwrites a previously persisted token', () => {
    savePersistedRefreshToken('refresh-1');
    savePersistedRefreshToken('refresh-2');
    expect(loadPersistedRefreshToken()).toBe('refresh-2');
  });

  it('clears the persisted token', () => {
    savePersistedRefreshToken('refresh-abc');
    clearPersistedRefreshToken();
    expect(loadPersistedRefreshToken()).toBeNull();
  });

  it('persists under sessionStorage, not localStorage (tab-lifetime, not indefinite)', () => {
    savePersistedRefreshToken('refresh-abc');
    expect(window.sessionStorage.getItem('markhand.refreshToken')).toBe('refresh-abc');
    expect(window.localStorage.getItem('markhand.refreshToken')).toBeNull();
  });
});
