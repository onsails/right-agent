import { describe, expect, it } from 'vitest'

import { identityLabel, identityTone } from './identityLabels'

describe('identityLabels', () => {
  it('maps each state to a human label', () => {
    expect(identityLabel('sandbox')).toBe('Live')
    expect(identityLabel('not_authored')).toBe('Not authored yet')
    expect(identityLabel('host_mirror')).toBe('Host mirror')
    expect(identityLabel('sandbox_unreachable')).toBe('Sandbox unreachable')
    expect(identityLabel('host')).toBe('Host')
    expect(identityLabel('missing')).toBe('Missing')
  })
  it('falls back to the raw value for unknown states', () => {
    expect(identityLabel('weird')).toBe('weird')
  })
  it('uses a warning tone only for unreachable/missing', () => {
    expect(identityTone('sandbox')).toBe('ok')
    expect(identityTone('host')).toBe('ok')
    expect(identityTone('host_mirror')).toBe('muted')
    expect(identityTone('not_authored')).toBe('muted')
    expect(identityTone('sandbox_unreachable')).toBe('bad')
    expect(identityTone('missing')).toBe('bad')
  })
})
