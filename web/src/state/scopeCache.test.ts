import { describe, expect, it } from 'vitest';
import { createScopeManager, type Scope } from './scope';
import { createScopeCache } from './scopeCache';

function scope(orgId: string): Scope {
  return { orgId, permissions: [], allowedCollectionIds: [] };
}

describe('createScopeCache', () => {
  it('serves what was set for the current epoch', () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const { epoch } = manager.getSnapshot();
    const cache = createScopeCache<string>(manager);

    cache.set(epoch, 'collections', 'org-a-collections');

    expect(cache.get('collections')).toBe('org-a-collections');
    expect(cache.has('collections')).toBe(true);
  });

  it('does not serve org A entries while org B is current', () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const epochA = manager.getSnapshot().epoch;
    const cache = createScopeCache<string>(manager);
    cache.set(epochA, 'collections', 'org-a-collections');

    manager.setScope(scope('org-b'));

    expect(cache.get('collections')).toBeUndefined();
    expect(cache.has('collections')).toBe(false);
  });

  it('discards a write for an epoch that is no longer current (late response landing after a switch)', () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const epochA = manager.getSnapshot().epoch;
    const cache = createScopeCache<string>(manager);

    // Switch happens *before* the org-A fetch resolves and tries to write.
    manager.setScope(scope('org-b'));
    cache.set(epochA, 'collections', 'org-a-collections-arriving-late');

    expect(cache.get('collections')).toBeUndefined();
  });

  it('switching back to org A does not resurrect org A pre-switch data', () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const epochA1 = manager.getSnapshot().epoch;
    const cache = createScopeCache<string>(manager);
    cache.set(epochA1, 'collections', 'org-a-collections-first-visit');

    manager.setScope(scope('org-b'));
    manager.setScope(scope('org-a')); // back to A, but a new generation

    expect(cache.get('collections')).toBeUndefined();

    const epochA2 = manager.getSnapshot().epoch;
    expect(epochA2).not.toBe(epochA1);
    cache.set(epochA2, 'collections', 'org-a-collections-second-visit');
    expect(cache.get('collections')).toBe('org-a-collections-second-visit');
  });

  it('accepts a write for a same-org refresh that does not bump the epoch', () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const cache = createScopeCache<string>(manager);
    const { epoch } = manager.getSnapshot();

    cache.set(epoch, 'collections', 'v1');
    manager.setScope(scope('org-a')); // same org: no epoch bump
    cache.set(manager.getSnapshot().epoch, 'collections', 'v2');

    expect(cache.get('collections')).toBe('v2');
  });

  it('clear() drops everything regardless of scope', () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const cache = createScopeCache<string>(manager);
    cache.set(manager.getSnapshot().epoch, 'k', 'v');
    cache.clear();
    expect(cache.get('k')).toBeUndefined();
  });

  it('delete() removes a single key without touching others', () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const cache = createScopeCache<string>(manager);
    const { epoch } = manager.getSnapshot();
    cache.set(epoch, 'a', '1');
    cache.set(epoch, 'b', '2');
    cache.delete('a');
    expect(cache.get('a')).toBeUndefined();
    expect(cache.get('b')).toBe('2');
  });
});
