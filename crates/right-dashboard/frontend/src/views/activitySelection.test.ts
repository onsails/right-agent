import { describe, expect, it } from 'vitest'

import type { OverviewResponse } from '../types'
import { activityContainsRun, isSameRunSelected } from './activitySelection'

function activity(runIds: string[]): OverviewResponse {
  return {
    crons: [{ recent_runs: runIds.map((id) => ({ id })) }],
  } as unknown as OverviewResponse
}

describe('activityContainsRun', () => {
  it('finds a run present in a cron recent_runs list', () => {
    expect(activityContainsRun(activity(['r1', 'r2']), 'r2')).toBe(true)
  })

  it('returns false when the run is absent', () => {
    expect(activityContainsRun(activity(['r1']), 'r9')).toBe(false)
  })

  it('returns false for null activity or null id', () => {
    expect(activityContainsRun(null, 'r1')).toBe(false)
    expect(activityContainsRun(activity(['r1']), null)).toBe(false)
  })
})

describe('isSameRunSelected', () => {
  it('is true only when the clicked run equals the currently selected run', () => {
    expect(isSameRunSelected('r1', 'r1')).toBe(true)
    expect(isSameRunSelected('r1', 'r2')).toBe(false)
    expect(isSameRunSelected(null, 'r1')).toBe(false)
  })
})
