import { describe, expect, it } from 'vitest'

import { failureSampleLabel } from './failureSampleLabel'

describe('failureSampleLabel', () => {
  it('returns null when the full set is shown', () => {
    expect(failureSampleLabel(3, 3)).toBeNull()
    expect(failureSampleLabel(2, 5)).toBeNull()
    expect(failureSampleLabel(0, 0)).toBeNull()
  })
  it('labels the sample when the total exceeds the shown rows', () => {
    expect(failureSampleLabel(137, 50)).toBe('latest 50 of 137')
  })
})
