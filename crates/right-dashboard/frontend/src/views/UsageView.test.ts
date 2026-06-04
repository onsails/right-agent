import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import UsageView from './UsageView.vue'
import type { UsageOverviewResponse, UsageSourceSummary, UsageWindow } from '../types'

function sourceSummaryStub(overrides: Partial<UsageSourceSummary> = {}): UsageSourceSummary {
  return {
    source: 'worker',
    cost_usd: 1.0,
    subscription_cost_usd: 0,
    api_cost_usd: 1.0,
    turns: 5,
    invocations: 5,
    input_tokens: 1000,
    output_tokens: 500,
    cache_creation_tokens: 200,
    cache_read_tokens: 800,
    web_search_requests: 0,
    web_fetch_requests: 0,
    per_model: [],
    ...overrides,
  }
}

function windowStub(overrides: Partial<UsageWindow> = {}): UsageWindow {
  return {
    key: '7d',
    label: 'Last 7 days',
    range_start: '2026-01-01T00:00:00Z',
    range_end: '2026-01-08T00:00:00Z',
    range_label: 'Jan 1 00:00-Jan 8 00:00',
    sources: [sourceSummaryStub()],
    total_cost_usd: 1.0,
    subscription_cost_usd: 0,
    api_cost_usd: 1.0,
    turns: 5,
    invocations: 5,
    input_tokens: 1000,
    output_tokens: 500,
    cache_creation_tokens: 200,
    cache_read_tokens: 800,
    web_search_requests: 0,
    web_fetch_requests: 0,
    per_model: [],
    budget_skip_count: 0,
    ...overrides,
  }
}

function usageStub(overrides: Partial<UsageOverviewResponse> = {}): UsageOverviewResponse {
  return {
    agent: 'test-agent',
    generated_at: '2026-01-01T00:00:00Z',
    timezone: 'UTC',
    windows: [windowStub()],
    selected_window: '7d',
    daily_series: [],
    source_series: [],
    warnings: [],
    ...overrides,
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(UsageView, props as any),
  })
  return renderToString(app)
}

describe('UsageView budget_skip_count', () => {
  it('renders budget-skip line when count > 0', async () => {
    const html = await render({
      usage: usageStub({ windows: [windowStub({ budget_skip_count: 3 })] }),
      loading: false,
      error: null,
    })
    expect(html).toContain('Budget-blocked learning attempts')
    expect(html).toContain('3')
  })

  it('does not render budget-skip line when count is 0', async () => {
    const html = await render({
      usage: usageStub({ windows: [windowStub({ budget_skip_count: 0 })] }),
      loading: false,
      error: null,
    })
    expect(html).not.toContain('Budget-blocked learning attempts')
  })
})

describe('UsageView token legend and per-source TokenLine', () => {
  it('renders the token legend and a per-source token line', async () => {
    const html = await render({
      usage: usageStub({
        windows: [windowStub({
          sources: [sourceSummaryStub({ source: 'interactive', cache_read_tokens: 300, cache_creation_tokens: 50 })],
        })],
      }),
      loading: false,
      error: null,
    })
    expect(html).toContain('token-legend')
    expect(html).toContain('token-line')
    expect(html).toContain('interactive')
    // New legend exposes cache via stable marker classes.
    expect(html).toContain('lg-create')
    expect(html).toContain('lg-read')
    // Old CacheSubline component (class `cache-subline`) is gone.
    expect(html).not.toContain('cache-subline')
  })
})
