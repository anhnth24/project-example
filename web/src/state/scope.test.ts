import { describe, expect, it, vi } from 'vitest';
import { createScopeManager, type Scope } from './scope';

function scope(orgId: string, overrides: Partial<Scope> = {}): Scope {
  return { orgId, permissions: [], allowedCollectionIds: [], ...overrides };
}

describe('createScopeManager', () => {
  describe('initial state', () => {
    it('starts anonymous at epoch 0', () => {
      const manager = createScopeManager();
      expect(manager.getSnapshot()).toEqual({ epoch: 0, scope: null });
      expect(manager.isCurrent(0)).toBe(true);
    });
  });

  describe('epoch bumping', () => {
    it('bumps the epoch on login (null -> a scope)', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      const snapshot = manager.getSnapshot();
      expect(snapshot.epoch).toBe(1);
      expect(snapshot.scope?.orgId).toBe('org-a');
    });

    it('bumps the epoch on an org switch (org A -> org B)', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      manager.setScope(scope('org-b'));
      expect(manager.getSnapshot()).toMatchObject({ epoch: 2, scope: { orgId: 'org-b' } });
    });

    it('bumps the epoch on logout (a scope -> null)', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      manager.setScope(null);
      expect(manager.getSnapshot()).toEqual({ epoch: 2, scope: null });
    });

    it('bumps the epoch when switching back to a previously-active org (A -> B -> A)', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      const epochA1 = manager.getSnapshot().epoch;
      manager.setScope(scope('org-b'));
      manager.setScope(scope('org-a'));
      const epochA2 = manager.getSnapshot().epoch;
      expect(epochA2).not.toBe(epochA1);
      expect(epochA2).toBe(3);
    });

    it('does NOT bump the epoch when re-setting the same org (e.g. a /auth/me refresh)', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a', { permissions: ['read'] }));
      const epochBefore = manager.getSnapshot().epoch;
      manager.setScope(scope('org-a', { permissions: ['read', 'write'] }));
      const snapshot = manager.getSnapshot();
      expect(snapshot.epoch).toBe(epochBefore);
      // But the scope payload itself is still refreshed.
      expect(snapshot.scope?.permissions).toEqual(['read', 'write']);
    });

    it('never reuses an epoch value', () => {
      const manager = createScopeManager();
      const seen = new Set<number>([manager.getSnapshot().epoch]);
      for (const org of ['org-a', 'org-b', 'org-a', 'org-c', 'org-a']) {
        manager.setScope(scope(org));
        const epoch = manager.getSnapshot().epoch;
        expect(seen.has(epoch)).toBe(false);
        seen.add(epoch);
      }
    });
  });

  describe('subscribe', () => {
    it('notifies listeners on every setScope call, including same-org refreshes', () => {
      const manager = createScopeManager();
      const listener = vi.fn();
      manager.subscribe(listener);

      manager.setScope(scope('org-a'));
      manager.setScope(scope('org-a', { permissions: ['x'] }));
      manager.setScope(scope('org-b'));

      expect(listener).toHaveBeenCalledTimes(3);
    });

    it('stops notifying after unsubscribe', () => {
      const manager = createScopeManager();
      const listener = vi.fn();
      const unsubscribe = manager.subscribe(listener);
      unsubscribe();

      manager.setScope(scope('org-a'));
      expect(listener).not.toHaveBeenCalled();
    });
  });

  describe('registerAbortable', () => {
    it('does not abort an abortable registered for the current epoch', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      const { epoch } = manager.getSnapshot();
      const abortable = { abort: vi.fn() };

      manager.registerAbortable(epoch, abortable);
      expect(abortable.abort).not.toHaveBeenCalled();
    });

    it('aborts every abortable registered under the previous epoch the instant the scope switches', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      const { epoch: epochA } = manager.getSnapshot();

      const request1 = { abort: vi.fn() };
      const request2 = { abort: vi.fn() };
      manager.registerAbortable(epochA, request1);
      manager.registerAbortable(epochA, request2);

      manager.setScope(scope('org-b'));

      expect(request1.abort).toHaveBeenCalledTimes(1);
      expect(request2.abort).toHaveBeenCalledTimes(1);
    });

    it('aborts immediately (synchronously) if the epoch is already stale when registering', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      const staleEpoch = manager.getSnapshot().epoch;
      manager.setScope(scope('org-b'));

      const lateRegistration = { abort: vi.fn() };
      manager.registerAbortable(staleEpoch, lateRegistration);

      expect(lateRegistration.abort).toHaveBeenCalledTimes(1);
    });

    it('does not abort an abortable belonging to a *different, still-current* epoch when a same-org refresh happens', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      const { epoch } = manager.getSnapshot();
      const abortable = { abort: vi.fn() };
      manager.registerAbortable(epoch, abortable);

      manager.setScope(scope('org-a', { permissions: ['x'] })); // same org: no epoch bump

      expect(abortable.abort).not.toHaveBeenCalled();
    });

    it('honors unregister: an abortable that finished on its own is not aborted again on a later switch', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      const { epoch } = manager.getSnapshot();
      const abortable = { abort: vi.fn() };
      const unregister = manager.registerAbortable(epoch, abortable);

      unregister();
      manager.setScope(scope('org-b'));

      expect(abortable.abort).not.toHaveBeenCalled();
    });
  });

  describe('isCurrent', () => {
    it('is true only for the epoch that is currently active', () => {
      const manager = createScopeManager();
      manager.setScope(scope('org-a'));
      const epochA = manager.getSnapshot().epoch;
      manager.setScope(scope('org-b'));
      const epochB = manager.getSnapshot().epoch;

      expect(manager.isCurrent(epochA)).toBe(false);
      expect(manager.isCurrent(epochB)).toBe(true);
    });
  });
});
