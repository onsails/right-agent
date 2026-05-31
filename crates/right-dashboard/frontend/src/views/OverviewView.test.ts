import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import OverviewView from './OverviewView.vue'

function overview(recentFailures: number, failedRuns: unknown[]) {
  return {
    agent: 'a', generated_at: '2026-05-31T12:00:00Z',
    active_runs: 0, recent_failures: recentFailures, today_cost_usd: 0,
    learning_candidates_24h: 0,
    doctor: { state: 'ok', pass_count: 1, warn_count: 0, fail_count: 0, generated_at: null },
    sandbox: { state: 'ok', detail: null },
    signals: [], cost_learning_river: null, warnings: [],
    recent_failed_runs: failedRuns,
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(OverviewView, props as any),
  })
  return renderToString(app)
}

// Grabs the opening tag (article|button) + its attributes for the element
// wrapping the "Failures" metric label. data-v scoped attrs are tolerated.
function failuresCard(html: string): { tag: string; attrs: string } {
  const m = html.match(/<(article|button)([^>]*)>\s*<span[^>]*>Failures<\/span>/)
  if (!m) throw new Error('Failures card not found in rendered HTML')
  return { tag: m[1], attrs: m[2] }
}

describe('OverviewView failures card', () => {
  it('renders the zero failures card gray + non-interactive (not green ok, not red bad)', async () => {
    const html = await render({ overview: overview(0, []), activity: null, loading: false, error: null })
    const card = failuresCard(html)
    expect(card.tag).toBe('article') // static, not an interactive <button>
    expect(card.attrs).toMatch(/\bdefault\b/) // neutral gray tone
    expect(card.attrs).not.toMatch(/\bok\b/) // NOT the old green "ok" tone
    expect(card.attrs).not.toMatch(/\bbad\b/) // NOT red at zero
    // interactive affordance is unique to the failures card — absent at zero
    expect(html).not.toContain('metric-card-interactive')
    // no drill-down section at zero (CollapsibleSection emits a count-badge)
    expect(html).not.toContain('count-badge')
  })
  it('renders the non-zero failures card as a red interactive button and reveals the list', async () => {
    const failed = [{ id: 'run-1', kind: 'cron', producer_ref: 'job', status: 'failed', started_at: null, finished_at: '2026-05-31T11:00:00Z', exit_code: 1, delivery_required: false, delivery_status: 'none', delivery_kind: null, run_note: null, cost_usd: 0 }]
    const html = await render({ overview: overview(1, failed), activity: null, loading: false, error: null })
    const card = failuresCard(html)
    expect(card.tag).toBe('button') // interactive
    expect(card.attrs).toMatch(/metric-card-interactive/) // clickable affordance
    expect(card.attrs).toMatch(/\bbad\b/) // red when there are failures
    // drill-down section is present (CollapsibleSection) with the failed run row
    expect(html).toContain('count-badge')
    expect(html).toContain('cron')
  })
})
