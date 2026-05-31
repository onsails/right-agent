export interface CacheTokens {
  input_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}

// Hit-rate = cache reads over all input-bearing tokens. Matches the old
// Telegram /usage format_cache_line definition (right-agent usage/format.rs).
export function cacheHitRate(t: CacheTokens): number {
  const bearing = t.input_tokens + t.cache_creation_tokens + t.cache_read_tokens
  return bearing === 0 ? 0 : t.cache_read_tokens / bearing
}
