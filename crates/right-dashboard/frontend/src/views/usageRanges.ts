import type { UsageRange } from '../types'

export const DEFAULT_USAGE_RANGE: UsageRange = 'last_7_days'

export const USAGE_RANGE_OPTIONS: Array<{ key: UsageRange, label: string }> = [
  { key: 'today', label: 'Today' },
  { key: 'last_3_days', label: '3 days' },
  { key: 'last_7_days', label: '7 days' },
  { key: 'last_30_days', label: '30 days' },
  { key: 'all_time', label: 'All time' },
]

export function isUsageRange(value: string | null | undefined): value is UsageRange {
  return USAGE_RANGE_OPTIONS.some((option) => option.key === value)
}
