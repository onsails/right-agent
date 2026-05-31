import { describe, expect, it } from 'vitest'

import { cacheHitRate } from './usageCache'

describe('cacheHitRate', () => {
  it('returns 0 when there are no input-bearing tokens', () => {
    expect(cacheHitRate({ input_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0 })).toBe(0)
  })
  it('computes reads over (input + creation + read)', () => {
    // 300 / (10 + 50 + 300) = 300/360 ≈ 0.8333 → 83% via percent()
    expect(cacheHitRate({ input_tokens: 10, cache_creation_tokens: 50, cache_read_tokens: 300 })).toBeCloseTo(0.8333, 4)
  })
  it('returns exactly 1 for pure cache-read input', () => {
    expect(cacheHitRate({ input_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 500 })).toBe(1)
  })
  it('returns 0 for uncached input only', () => {
    expect(cacheHitRate({ input_tokens: 100, cache_creation_tokens: 0, cache_read_tokens: 0 })).toBe(0)
  })
})
