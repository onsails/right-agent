import type { CronCard } from '../types'

export type CronSortMode = 'name' | 'spend_24h' | 'spend_7d'

export const CRON_SORT_MODES: { value: CronSortMode; label: string }[] = [
  { value: 'name', label: 'Name' },
  { value: 'spend_24h', label: 'Spend 24h' },
  { value: 'spend_7d', label: 'Spend 7d' },
]

export function sortCrons(crons: CronCard[], mode: CronSortMode): CronCard[] {
  const copy = [...crons]
  const byName = (a: CronCard, b: CronCard) => a.job_name.localeCompare(b.job_name)
  switch (mode) {
    case 'spend_24h':
      return copy.sort((a, b) => b.spend_24h_usd - a.spend_24h_usd || byName(a, b))
    case 'spend_7d':
      return copy.sort((a, b) => b.spend_7d_usd - a.spend_7d_usd || byName(a, b))
    default:
      return copy.sort(byName)
  }
}
