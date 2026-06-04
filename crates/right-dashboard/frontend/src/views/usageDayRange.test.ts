import { describe, expect, it } from 'vitest'
import { selectedDayRangeLabel } from './usageDayRange'

describe('selectedDayRangeLabel', () => {
  it('formats past local days as full-day ranges', () => {
    expect(selectedDayRangeLabel('2026-06-03', 'Asia/Dubai', '2026-06-04T16:47:36Z'))
      .toBe('Asia/Dubai · Jun 3 00:00-23:59')
  })

  it('formats the current local day through generated-at local time', () => {
    expect(selectedDayRangeLabel('2026-06-04', 'Asia/Dubai', '2026-06-04T16:47:36Z'))
      .toBe('Asia/Dubai · Jun 4 00:00-20:47')
  })

  it('returns null when no date is selected', () => {
    expect(selectedDayRangeLabel(null, 'Asia/Dubai', '2026-06-04T16:47:36Z')).toBeNull()
  })
})
