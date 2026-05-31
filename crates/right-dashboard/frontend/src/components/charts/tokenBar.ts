import { inputBearing, type CacheTokens } from './usageCache'

// Token counts for one row of usage (a daily point, a source, or a window).
// Extends the input-bearing trio (CacheTokens) with generation output;
// `cacheHitRate`/`inputBearing` consume the inherited subset.
export interface TokenCounts extends CacheTokens {
  output_tokens: number
}

// Stacked widths (fractions summing to 1) for the input-bearing trio.
export interface HitSegments {
  miss: number
  create: number
  read: number
}

// Minimum visible width for any nonzero segment, so a 0.5% miss/create does
// not vanish under a dominant cache_read. Tuning constant.
const DEFAULT_MIN_WIDTH = 0.04

// Returns null when there are no input-bearing tokens (caller hides the bar).
export function hitSegments(t: TokenCounts, minWidth: number = DEFAULT_MIN_WIDTH): HitSegments | null {
  const bearing = inputBearing(t)
  if (bearing === 0) {
    return null
  }

  const keys = ['miss', 'create', 'read'] as const
  const result: HitSegments = {
    miss: t.input_tokens / bearing,
    create: t.cache_creation_tokens / bearing,
    read: t.cache_read_tokens / bearing,
  }

  // Bump nonzero segments below the floor up to minWidth; remove the overflow
  // from donor segments above the floor, proportional to their slack.
  const bumped = keys.filter((k) => result[k] > 0 && result[k] < minWidth)
  if (bumped.length === 0) {
    return result
  }

  let deficit = 0
  for (const k of bumped) {
    deficit += minWidth - result[k]
    result[k] = minWidth
  }
  const donors = keys.filter((k) => result[k] > minWidth)
  const slack = donors.reduce((sum, k) => sum + (result[k] - minWidth), 0)
  // Precondition: minWidth < 1/3. With at most 3 segments summing to 1, total
  // floor demand is < 1, so a donor with positive slack always exists here and
  // the result sums to 1. Raising minWidth past 1/3 breaks that invariant —
  // the slack guard would then silently under-fill the bar.
  if (slack > 0) {
    for (const k of donors) {
      result[k] -= deficit * ((result[k] - minWidth) / slack)
    }
  }
  return result
}
