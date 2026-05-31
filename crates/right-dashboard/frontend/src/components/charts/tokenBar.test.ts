import { describe, expect, it } from 'vitest'

import { hitSegments, type TokenCounts } from './tokenBar'

function counts(over: Partial<TokenCounts> = {}): TokenCounts {
  return { input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0, ...over }
}

describe('hitSegments', () => {
  it('returns null when there are no input-bearing tokens', () => {
    // output alone is not input-bearing.
    expect(hitSegments(counts({ output_tokens: 99 }))).toBeNull()
  })

  it('returns raw proportions summing to 1 when every segment clears the floor', () => {
    const s = hitSegments(counts({ input_tokens: 100, cache_creation_tokens: 100, cache_read_tokens: 100 }))!
    expect(s.miss).toBeCloseTo(1 / 3, 6)
    expect(s.create).toBeCloseTo(1 / 3, 6)
    expect(s.read).toBeCloseTo(1 / 3, 6)
    expect(s.miss + s.create + s.read).toBeCloseTo(1, 6)
  })

  it('bumps a tiny nonzero segment to the floor and still sums to 1', () => {
    // bearing 360; raw miss = 10/360 ≈ 0.0278 < 0.04 floor.
    const s = hitSegments(counts({ input_tokens: 10, cache_creation_tokens: 50, cache_read_tokens: 300 }))!
    expect(s.miss).toBeCloseTo(0.04, 6)
    expect(s.miss + s.create + s.read).toBeCloseTo(1, 6)
    expect(s.read).toBeLessThan(300 / 360) // donor gave up width
    expect(s.read).toBeGreaterThan(s.create)
  })

  it('bumps two tiny segments and renormalizes via the single donor', () => {
    const s = hitSegments(counts({ input_tokens: 5, cache_creation_tokens: 5, cache_read_tokens: 990 }))!
    expect(s.miss).toBeCloseTo(0.04, 6)
    expect(s.create).toBeCloseTo(0.04, 6)
    expect(s.read).toBeCloseTo(0.92, 6)
    expect(s.miss + s.create + s.read).toBeCloseTo(1, 6)
  })

  it('keeps zero segments at zero', () => {
    const s = hitSegments(counts({ cache_read_tokens: 500 }))!
    expect(s.miss).toBe(0)
    expect(s.create).toBe(0)
    expect(s.read).toBe(1)
  })

  it('leaves a lone segment at full width (never below floor)', () => {
    const s = hitSegments(counts({ input_tokens: 100 }))!
    expect(s.miss).toBe(1)
    expect(s.create).toBe(0)
    expect(s.read).toBe(0)
  })
})
