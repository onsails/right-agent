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

describe('UsageView cache tokens per source', () => {
  it('renders cache read/creation tokens in source rows', async () => {
    const html = await render({
      usage: usageStub({
        windows: [windowStub({
          sources: [sourceSummaryStub({ cache_read_tokens: 1234, cache_creation_tokens: 567 })],
        })],
      }),
      loading: false,
      error: null,
    })
    expect(html).toContain('1234')
    expect(html).toContain('567')
  })
})
