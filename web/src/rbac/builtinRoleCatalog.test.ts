import { describe, expect, it } from 'vitest'
import { BUILTIN_ROLE_CATALOG, ROLE_ORDER } from './builtinRoleCatalog'

describe('builtin role catalog', () => {
  it('drives role ordering from the canonical fixture', () => {
    expect(ROLE_ORDER).toEqual(BUILTIN_ROLE_CATALOG.roles)
  })
  it('keeps reserved permissions ungranted', () => {
    const reserved = new Set(
      BUILTIN_ROLE_CATALOG.permissions
        .filter((permission) => permission.status === 'reserved')
        .map((permission) => permission.key),
    )
    expect(Object.values(BUILTIN_ROLE_CATALOG.grants).flat().some((key) => reserved.has(key))).toBe(false)
  })
})
