import { describe, expect, it } from 'vitest'

import { shouldTick } from './useLiveResource'

describe('shouldTick', () => {
  it('ticks when visible, idle, and not paused', () => {
    expect(shouldTick({ hidden: false, inFlight: false, pauseWhenHidden: true })).toBe(true)
  })

  it('skips while a fetch is already in flight', () => {
    expect(shouldTick({ hidden: false, inFlight: true, pauseWhenHidden: true })).toBe(false)
  })

  it('skips while hidden when pause-when-hidden is on', () => {
    expect(shouldTick({ hidden: true, inFlight: false, pauseWhenHidden: true })).toBe(false)
  })

  it('keeps ticking while hidden when pause-when-hidden is off', () => {
    expect(shouldTick({ hidden: true, inFlight: false, pauseWhenHidden: false })).toBe(true)
  })
})
