import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import ReportsView from './ReportsView.vue'

function learning(over: Record<string, unknown> = {}) {
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
      failed_7d: 0,
      refused_7d: 0,
      recent_successful_events: [],
      recent_failed_events: [],
      recent_refused_events: [],
      candidate_skill_names_7d: [],
      ...over,
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

function failedCard(html: string): { tag: string; attrs: string } {
  const match = html.match(/<(article|button)([^>]*)>\s*<span[^>]*>Failed 7d<\/span>/)
  if (!match) {
    throw new Error('Failed 7d card not found')
  }
  return { tag: match[1], attrs: match[2] }
}

describe('ReportsView learning layout', () => {
  it('renders Failed skills panel always visible and non-interactive Failed card', async () => {
    const html = await render({ learning: learning() })
    const card = failedCard(html)
    expect(html).toContain('Failed skills')
    expect(html).toContain('A failed skill is a learning attempt that errored out')
    expect(card.tag).toBe('article')
    expect(card.attrs).not.toContain('metric-card-interactive')
    expect(html).not.toContain('count-badge')
  })

  it('lists failed events and a refusals caption', async () => {
    const html = await render({
      learning: learning({
        failed_7d: 1,
        refused_7d: 5,
        recent_failed_events: [{
          id: 1,
          skill_name: 'rightx-a', action: 'update', status: 'failed',
          message: 'boom', summary: null, created_at: '2026-05-31T11:00:00Z',
        }],
        recent_refused_events: [{
          id: 2,
          skill_name: 'rightx-refused', action: 'update', status: 'aborted',
          message: 'already covered', summary: null, created_at: '2026-05-31T11:01:00Z',
        }],
      }),
    })
    const card = failedCard(html)
    expect(card.tag).toBe('article')
    expect(card.attrs).not.toContain('metric-card-interactive')
    expect(card.attrs).toMatch(/\bbad\b/)
    expect(html).toContain('rightx-a')
    expect(html).not.toContain('rightx-refused')
    expect(html).toContain('Refused 5')
  })

  it('hides the refusals caption when there are none', async () => {
    const html = await render({ learning: learning({ refused_7d: 0 }) })
    expect(html).not.toContain('Refused 0')
  })
})
