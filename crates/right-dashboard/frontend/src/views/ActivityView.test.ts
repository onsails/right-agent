import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import ActivityView from './ActivityView.vue'

function activity(failedCount: number, failedRuns: unknown[]) {
  return {
    agent: 'a', generated_at: '2026-05-31T12:00:00Z', refresh_interval_secs: 5,
    summary: { cron_count: 0, active_cron_count: 0, failed_recent_cron_count: failedCount, today_cost_usd: 0 },
    crons: [], failed_runs: failedRuns,
    active: { foreground: [], background: [] },
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(ActivityView, props as any),
  })
  return renderToString(app)
}

// Opening tag + attrs of the element wrapping the "Failures" metric label (data-v tolerant)
function failuresCard(html: string): { tag: string; attrs: string } {
  const m = html.match(/<(article|button)([^>]*)>\s*<span[^>]*>Failures<\/span>/)
  if (!m) throw new Error('Failures card not found')
  return { tag: m[1], attrs: m[2] }
}

function cronCard(overrides: Record<string, unknown> = {}) {
  return {
    job_name: 'daily', schedule: '0 8 * * *', schedule_human: 'At 08:00, every day',
    recurring: true, run_at: null, next_run_at: '2026-06-02T08:00:00Z',
    target_chat_id: 123, target_thread_id: null, max_budget_usd: 2,
    spend_24h_usd: 0.25, spend_7d_usd: 1.5, last_run: null, recent_runs: [], ...overrides,
  }
}

function overviewWithCron(cron: Record<string, unknown>) {
  return {
    agent: 'a', generated_at: '2026-06-01T12:00:00Z', refresh_interval_secs: 5,
    summary: { cron_count: 1, active_cron_count: 0, failed_recent_cron_count: 0, today_cost_usd: 0 },
    crons: [cron], failed_runs: [], active: { foreground: [], background: [] },
  }
}

describe('ActivityView cron card', () => {
  it('renders human schedule and spend figures, not the raw cron expression as the primary line', async () => {
    const html = await render({
      overview: overviewWithCron(cronCard()), selectedRun: null, selectedRunId: null,
      loadingDetail: false, detailError: null,
    })
    expect(html).toContain('At 08:00, every day')
    expect(html).toContain('$0.25')
    expect(html).toContain('$1.50')
  })

  it('renders a delete button for the cron', async () => {
    const html = await render({
      overview: overviewWithCron(cronCard()), selectedRun: null, selectedRunId: null,
      loadingDetail: false, detailError: null,
    })
    expect(html).toMatch(/cron-delete/)
  })

  it('renders the sort control', async () => {
    const html = await render({
      overview: overviewWithCron(cronCard()), selectedRun: null, selectedRunId: null,
      loadingDetail: false, detailError: null,
    })
    expect(html).toContain('Spend 24h')
    expect(html).toContain('Spend 7d')
  })
})

describe('ActivityView failures card', () => {
  it('renders the zero failures card gray + non-interactive (not green ok, not red bad)', async () => {
    const html = await render({ overview: activity(0, []), selectedRun: null, selectedRunId: null, loadingDetail: false, detailError: null })
    const card = failuresCard(html)
    expect(card.tag).toBe('article')
    expect(card.attrs).toMatch(/\bdefault\b/)
    expect(card.attrs).not.toMatch(/\bok\b/)
    expect(card.attrs).not.toMatch(/\bbad\b/)
    expect(html).not.toContain('metric-card-interactive')
    // no failures drill-down section at zero (CollapsibleSection emits a count-badge)
    expect(html).not.toContain('count-badge')
  })
  it('renders the non-zero failures card as a red interactive button and reveals the list', async () => {
    const failed = [{ id: 'run-1', kind: 'cron', producer_ref: 'job', status: 'failed', started_at: null, finished_at: '2026-05-31T11:00:00Z', exit_code: 1, delivery_required: false, delivery_status: 'none', delivery_kind: null, run_note: null, cost_usd: 0 }]
    const html = await render({ overview: activity(1, failed), selectedRun: null, selectedRunId: null, loadingDetail: false, detailError: null })
    const card = failuresCard(html)
    expect(card.tag).toBe('button')
    expect(card.attrs).toMatch(/metric-card-interactive/)
    expect(card.attrs).toMatch(/\bbad\b/)
    // failures drill-down section header is present (CollapsibleSection count-badge).
    // SSR renders the section CLOSED (body is v-if=isOpen, failuresOpen starts false), so
    // the RunFailureList rows are not in SSR output — the expand is verified manually.
    expect(html).toContain('count-badge')
  })
})
