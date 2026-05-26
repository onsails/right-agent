import { describe, expect, it } from 'vitest'

import { secretInputType, secretToggleAriaLabel, secretToggleText } from './secretInputModel'

describe('SecretInput model behavior', () => {
  it('masks values until reveal is enabled', () => {
    expect(secretInputType(false)).toBe('password')
    expect(secretInputType(true)).toBe('text')
  })

  it('labels the reveal toggle by current state', () => {
    expect(secretToggleText(false)).toBe('Show')
    expect(secretToggleText(true)).toBe('Hide')
    expect(secretToggleAriaLabel(false)).toBe('Show value')
    expect(secretToggleAriaLabel(true)).toBe('Hide value')
  })
})
