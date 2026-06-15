import type { CuratorRunSummary } from '../../types'

export type Tone = 'ok' | 'bad' | 'info'

export function curatorRunStatusTone(status: string): Tone {
  if (status === 'failed') { return 'bad' }
  if (status === 'success') { return 'ok' }
  return 'info'
}

/** Prefer the backend summary; otherwise synthesise from the counts. */
export function curatorRunHeadline(run: CuratorRunSummary): string {
  if (run.summary && run.summary.length > 0) { return run.summary }
  return `merged ${run.consolidations}, archived ${run.archives}`
}
