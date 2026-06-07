import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import UsageView from './UsageView.vue'
import type { UsageCronJobSummary, UsageOverviewResponse, UsageRange, UsageSourceSummary, UsageWindow } from '../types'

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
    range_start: '2025-12-26T00:00:00+04:00',
    range_end: '2026-01-01T04:00:00+04:00',
    range_label: 'Asia/Dubai · Dec 26 00:00-Jan 1 04:00',
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

function cronJobStub(overrides: Partial<UsageCronJobSummary> = {}): UsageCronJobSummary {
  return {
    job_name: 'nightly-report',
    cost_usd: 2.5,
    subscription_cost_usd: 1.5,
    api_cost_usd: 1.0,
    turns: 3,
    invocations: 3,
    input_tokens: 1100,
    output_tokens: 600,
    cache_creation_tokens: 100,
    cache_read_tokens: 900,
    web_search_requests: 0,
    web_fetch_requests: 0,
    per_model: [],
    ...overrides,
  }
}

function usageStub(overrides: Partial<UsageOverviewResponse> = {}): UsageOverviewResponse {
  return {
    agent: 'test-agent',
    generated_at: '2026-01-01T00:00:00Z',
    timezone: 'Asia/Dubai',
    selected_range: 'last_7_days',
    window: windowStub(),
    windows: [windowStub()],
    selected_window: '7d',
    daily_series: [],
    source_series: [],
    cron_jobs: [],
    warnings: [],
    ...overrides,
  }
}

async function render(props: Record<string, unknown>, selectedRange: UsageRange = 'last_7_days') {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(UsageView, { selectedRange, ...props } as any),
  })
  return renderToString(app)
}

describe('UsageView range selector', () => {
  it('renders one active selectable usage range', async () => {
    const html = await render({
      usage: usageStub(),
      loading: false,
      error: null,
    }, 'last_3_days')

    expect(html).toContain('aria-label="Usage range"')
    expect(html).toContain('Today')
    expect(html).toContain('3 days')
    expect(html).toContain('7 days')
    expect(html).toContain('30 days')
    expect(html).toContain('All time')
    expect((html.match(/aria-pressed="true"/g) ?? []).length).toBe(1)
  })
})

describe('UsageView budget_skip_count', () => {
  it('renders budget-skip line when count > 0', async () => {
    const html = await render({
      usage: usageStub({ window: windowStub({ budget_skip_count: 3 }) }),
      loading: false,
      error: null,
    })
    expect(html).toContain('Budget-blocked learning attempts')
    expect(html).toContain('3')
  })

  it('does not render budget-skip line when count is 0', async () => {
    const html = await render({
      usage: usageStub({ window: windowStub({ budget_skip_count: 0 }) }),
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
        window: windowStub({
          sources: [sourceSummaryStub({ source: 'interactive', cache_read_tokens: 300, cache_creation_tokens: 50 })],
        }),
      }),
      loading: false,
      error: null,
    })
    expect((html.match(/token-legend/g) ?? []).length).toBe(2)
    expect((html.match(/is-sticky/g) ?? []).length).toBe(1)
    expect(html).toContain('token-line')
    expect(html).toContain('interactive')
    expect(html).toContain('Asia/Dubai · Dec 26 00:00-Jan 1 04:00')
    expect(html).toContain('lg-create')
    expect(html).toContain('lg-read')
    expect(html).not.toContain('cache-subline')
  })

  it('passes the selected local day range to the breakdown panel', async () => {
    const html = await render({
      usage: usageStub({
        generated_at: '2026-06-04T16:47:36Z',
        timezone: 'Asia/Dubai',
        daily_series: [{
          date: '2026-06-04',
          total_cost_usd: 1,
          subscription_cost_usd: 1,
          api_cost_usd: 0,
          turns: 1,
          invocations: 1,
          input_tokens: 10,
          output_tokens: 20,
          cache_creation_tokens: 5,
          cache_read_tokens: 40,
          web_search_requests: 0,
          web_fetch_requests: 0,
          sources: [],
          models: [],
        }],
      }),
      loading: false,
      error: null,
    })

    expect(html).toContain('Asia/Dubai · Jun 4 00:00-20:47')
  })

  it('renders only the selected usage window, not every compatibility window', async () => {
    const html = await render({
      usage: usageStub({
        window: windowStub({ label: 'Selected window only', total_cost_usd: 7 }),
        windows: [windowStub({ key: 'unused', label: 'Unused compatibility window', total_cost_usd: 30 })],
      }),
      loading: false,
      error: null,
    })

    expect(html).toContain('Selected window only')
    expect(html).not.toContain('Unused compatibility window')
  })
})

describe('UsageView cron jobs', () => {
  it('renders cron job usage breakdown for the selected range', async () => {
    const html = await render({
      usage: usageStub({
        cron_jobs: [cronJobStub({
          job_name: 'nightly-usage-rollup',
          cost_usd: 2.5,
          per_model: [{
            model: 'claude-sonnet',
            cost_usd: 1.75,
            input_tokens: 700,
            output_tokens: 300,
            cache_creation_tokens: 50,
            cache_read_tokens: 400,
          }],
        })],
      }),
      loading: false,
      error: null,
    })

    expect(html).toContain('Cron jobs')
    expect(html).toContain('nightly-usage-rollup')
    expect(html).toContain('$2.50')
    expect(html).toContain('claude-sonnet')
  })

  it('renders an empty cron jobs state when no cron usage exists in the range', async () => {
    const html = await render({
      usage: usageStub({ cron_jobs: [] }),
      loading: false,
      error: null,
    })

    expect(html).toContain('No cron usage for period')
  })
})
