import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import ReportsView from './ReportsView.vue'

function learning(failed: number, failedEvents: unknown[]) {
  return {
    agent: 'a',
    generated_at: '2026-05-31T12:00:00Z',
    flow_nodes: [],
    flow_edges: [],
    recent_learning_signals: [],
    warnings: [],
    lifecycle: {
      created_7d: 0,
      updated_7d: 0,
      failed_or_aborted_7d: failed,
      recent_successful_events: [],
      candidate_skill_names_7d: [],
      recent_failed_events: failedEvents,
    },
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(ReportsView, props as any),
  })
  return renderToString(app)
}

// Opening tag + attrs of the element wrapping the "Failed 7d" metric label (data-v tolerant)
function failedCard(html: string): { tag: string; attrs: string } {
  const m = html.match(/<(article|button)([^>]*)>\s*<span[^>]*>Failed 7d<\/span>/)
  if (!m) throw new Error('Failed 7d card not found')
  return { tag: m[1], attrs: m[2] }
}

describe('ReportsView failed-skills card', () => {
  it('renders the zero Failed-7d card gray + non-interactive (not green ok, not red bad)', async () => {
    const html = await render({ learning: learning(0, []) })
    const card = failedCard(html)
    expect(card.tag).toBe('article')
    expect(card.attrs).toMatch(/\bdefault\b/)
    expect(card.attrs).not.toMatch(/\bok\b/)
    expect(card.attrs).not.toMatch(/\bbad\b/)
    expect(html).not.toContain('metric-card-interactive')
    // no failed-skills drill-down at zero (CollapsibleSection not mounted → no count-badge)
    expect(html).not.toContain('count-badge')
  })
  it('renders the non-zero Failed-7d card as a red interactive button and lists failed events', async () => {
    const events = [
      {
        skill_name: 'rightx',
        action: 'update',
        status: 'failed',
        message: 'boom',
        summary: null,
        created_at: '2026-05-31T11:00:00Z',
      },
    ]
    const html = await render({ learning: learning(1, events) })
    const card = failedCard(html)
    expect(card.tag).toBe('button')
    expect(card.attrs).toMatch(/metric-card-interactive/)
    expect(card.attrs).toMatch(/\bbad\b/)
    // failed-skills CollapsibleSection mounted (header always visible regardless of open state)
    expect(html).toContain('count-badge')
    expect(html).toContain('Failed skills')
  })
})
