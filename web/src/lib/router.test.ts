import { describe, expect, it } from 'vitest';
import { buildScopedPath, matchRoute } from './router';

describe('matchRoute', () => {
  it('matches the root path as home', () => {
    expect(matchRoute('/')).toEqual({ name: 'home', params: {} });
    expect(matchRoute('')).toEqual({ name: 'home', params: {} });
  });

  it('matches login', () => {
    expect(matchRoute('/login')).toEqual({ name: 'login', params: {} });
  });

  it('matches library with and without a collection id', () => {
    expect(matchRoute('/library')).toEqual({ name: 'library', params: {} });
    expect(matchRoute('/library/col-1')).toEqual({
      name: 'library',
      params: { collectionId: 'col-1' },
    });
  });

  it('decodes an encoded collection id', () => {
    expect(matchRoute('/qa/col%20one')).toEqual({
      name: 'qa',
      params: { collectionId: 'col one' },
    });
  });

  it('matches admin routes and does not let /library shadow /admin/*', () => {
    expect(matchRoute('/admin/members')).toEqual({ name: 'adminMembers', params: {} });
    expect(matchRoute('/admin/usage')).toEqual({ name: 'adminUsage', params: {} });
  });

  it('matches help', () => {
    expect(matchRoute('/help')).toEqual({ name: 'help', params: {} });
  });

  it('falls back to notFound for unknown paths', () => {
    expect(matchRoute('/does-not-exist')).toEqual({ name: 'notFound', params: {} });
    expect(matchRoute('/admin')).toEqual({ name: 'notFound', params: {} });
  });
});

describe('buildScopedPath', () => {
  it('builds a bare path without a collection id', () => {
    expect(buildScopedPath('library')).toBe('/library');
  });

  it('builds a scoped path and encodes the collection id', () => {
    expect(buildScopedPath('qa', 'col one')).toBe('/qa/col%20one');
  });
});
