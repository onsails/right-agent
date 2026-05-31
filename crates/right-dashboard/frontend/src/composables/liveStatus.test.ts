import { describe, expect, it } from 'vitest'

import { DashboardApiError } from '../api'
import { classifyOutcome, reduceConnectionState } from './liveStatus'

describe('reduceConnectionState', () => {
  it('returns null for an empty set', () => {
    expect(reduceConnectionState([])).toBeNull()
  })

  it('shows the worst state by priority locked > offline > stale > loading > live', () => {
    expect(reduceConnectionState(['live', 'live'])).toBe('live')
    expect(reduceConnectionState(['live', 'stale'])).toBe('stale')
    expect(reduceConnectionState(['offline', 'stale', 'live'])).toBe('offline')
    expect(reduceConnectionState(['locked', 'offline'])).toBe('locked')
    expect(reduceConnectionState(['loading', 'live'])).toBe('loading')
  })
})

describe('classifyOutcome', () => {
  it('maps a success to live', () => {
    expect(classifyOutcome({ ok: true, hasData: false })).toBe('live')
  })

  it('maps a 401/403 to locked regardless of data', () => {
    expect(classifyOutcome({ ok: false, error: new DashboardApiError('x', 401), hasData: true })).toBe('locked')
    expect(classifyOutcome({ ok: false, error: new DashboardApiError('x', 403), hasData: false })).toBe('locked')
  })

  it('keeps stale data on a non-auth failure, else offline', () => {
    expect(classifyOutcome({ ok: false, error: new DashboardApiError('x', 500), hasData: true })).toBe('stale')
    expect(classifyOutcome({ ok: false, error: new Error('network'), hasData: false })).toBe('offline')
  })
})
