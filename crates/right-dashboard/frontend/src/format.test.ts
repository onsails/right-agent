import { describe, expect, it } from 'vitest'

import { compactCount } from './format'

describe('compactCount', () => {
  it('passes small integers through unchanged', () => {
    expect(compactCount(0)).toBe('0')
    expect(compactCount(42)).toBe('42')
    expect(compactCount(999)).toBe('999')
  })
  it('uses a k suffix for thousands', () => {
    expect(compactCount(1_000)).toBe('1.0k')
    expect(compactCount(1_234)).toBe('1.2k')
  })
  it('uses an M suffix for millions', () => {
    expect(compactCount(1_000_000)).toBe('1.0M')
    expect(compactCount(1_234_567)).toBe('1.2M')
  })
})
