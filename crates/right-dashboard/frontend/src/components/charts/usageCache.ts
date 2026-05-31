export interface CacheTokens {
  input_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}

// Sum of all input-bearing tokens (raw input + cache writes + cache reads).
// The single definition of "input-bearing" — both cacheHitRate and the
// hit-bar segments in tokenBar.ts divide by this. Counts are u64-derived and
// therefore non-negative.
export function inputBearing(t: CacheTokens): number {
  return t.input_tokens + t.cache_creation_tokens + t.cache_read_tokens
}

// Hit-rate = cache reads over all input-bearing tokens. Matches the old
// Telegram /usage format_cache_line definition (right-agent usage/format.rs).
export function cacheHitRate(t: CacheTokens): number {
  const bearing = inputBearing(t)
  return bearing === 0 ? 0 : t.cache_read_tokens / bearing
}
