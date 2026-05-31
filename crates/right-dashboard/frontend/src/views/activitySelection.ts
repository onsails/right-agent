import type { OverviewResponse } from '../types'

export function activityContainsRun(activity: OverviewResponse | null, runId: string | null): boolean {
  if (activity === null || runId === null) {
    return false
  }
  return activity.crons.some((cron) => cron.recent_runs.some((run) => run.id === runId))
}

export function isSameRunSelected(selectedRunId: string | null, runId: string): boolean {
  return selectedRunId === runId
}
