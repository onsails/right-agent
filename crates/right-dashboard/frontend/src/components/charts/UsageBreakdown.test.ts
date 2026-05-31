import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import UsageBreakdown from './UsageBreakdown.vue'
import type { UsageDailyPoint } from '../../types'

function point(over: Partial<UsageDailyPoint> = {}): UsageDailyPoint {
  return {
    date: '2026-05-31', total_cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
    turns: 1, invocations: 1, input_tokens: 10, output_tokens: 20,
    cache_creation_tokens: 50, cache_read_tokens: 300,
    web_search_requests: 0, web_fetch_requests: 0,
    sources: [{
      source: 'interactive', cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
      turns: 1, invocations: 1, input_tokens: 10, output_tokens: 20,
      cache_creation_tokens: 50, cache_read_tokens: 300,
    }],
    models: [],
    ...over,
  }
}

async function render(p: UsageDailyPoint | null) {
  const app = createSSRApp({ render: () => h(UsageBreakdown, { point: p }) })
  return renderToString(app)
}

describe('UsageBreakdown tokens', () => {
  it('renders the per-day token line, hit-rate and a per-source token line', async () => {
    const html = await render(point())
    expect(html).toContain('token-line')
    expect(html).toContain('hit-bar')
    expect(html).toContain('83%')
    expect(html).toContain('interactive')
  })

  it('renders the new TokenLine cache segments in place of the old subline and Web row', async () => {
    const html = await render(point())
    // New intended markup: cache create/read live as TokenLine segments.
    expect(html).toContain('tok-create')
    expect(html).toContain('tok-read')
    expect(html).toContain('seg-create')
    expect(html).toContain('seg-read')
    // Old CacheSubline component (class `cache-subline`) is gone.
    expect(html).not.toContain('cache-subline')
    // Old `<span>Web</span>` counters row is gone.
    expect(html).not.toContain('<span>Web</span>')
  })

  it('omits a source hit bar when that source has no input-bearing tokens but keeps the per-day bar', async () => {
    const html = await render(point({
      sources: [{
        source: 'cron', cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
        turns: 1, invocations: 1, input_tokens: 0, output_tokens: 0,
        cache_creation_tokens: 0, cache_read_tokens: 0,
      }],
    }))
    expect(html).toContain('cron')
    expect(html).toContain('hit-bar') // per-day Counters still input-bearing
  })
})
