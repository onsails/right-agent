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
      turns: 1, invocations: 1, input_tokens: 10, cache_creation_tokens: 50, cache_read_tokens: 300,
    }],
    models: [],
    ...over,
  }
}

async function render(p: UsageDailyPoint | null) {
  const app = createSSRApp({ render: () => h(UsageBreakdown, { point: p }) })
  return renderToString(app)
}

describe('UsageBreakdown cache', () => {
  it('renders a per-source cache subline and a per-day hit-rate', async () => {
    const html = await render(point())
    expect(html).toContain('created')
    expect(html).toContain('83%')
    expect(html).toContain('hit')
  })
  it('omits the per-source subline when that source has no reads', async () => {
    const html = await render(point({
      sources: [{
        source: 'cron', cost_usd: 1, subscription_cost_usd: 1, api_cost_usd: 0,
        turns: 1, invocations: 1, input_tokens: 10, cache_creation_tokens: 0, cache_read_tokens: 0,
      }],
    }))
    expect(html).not.toContain('created')
    expect(html).toContain('cron')
    expect(html).toContain('83%') // per-day Counters hit-rate still renders (source subline is omitted)
    expect(html).toContain('hit')
  })
})
