import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import LearningSignalPanel from './LearningSignalPanel.vue'
import type { LearningSignalPoint } from '../../types'

function signal(over: Partial<LearningSignalPoint> = {}): LearningSignalPoint {
  return {
    id: 'learning:1',
    occurred_at: '2026-05-20T10:00:00Z',
    kind: 'skill_refused',
    label: 'rightx-twitter-content-drafter',
    severity: 'info',
    detail: 'Insufficient evidence.',
    skill_name: 'rightx-twitter-content-drafter',
    count: 1,
    ...over,
  }
}

async function render(signals: LearningSignalPoint[]): Promise<string> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const app = createSSRApp({ render: () => h(LearningSignalPanel, { signals } as any) })
  return renderToString(app)
}

describe('LearningSignalPanel', () => {
  it('shows the detail preview text', async () => {
    const html = await render([signal()])
    expect(html).toContain('Insufficient evidence.')
  })

  it('shows a human outcome label instead of a raw severity word', async () => {
    const html = await render([signal({ kind: 'skill_refused' })])
    expect(html).toContain('Refused')
  })

  it('colors a refused signal muted, never the active (amber) alert tone', async () => {
    const html = await render([signal({ severity: 'info' })])
    const tone = html.match(/class="status-pill ([a-z]+)"/)
    expect(tone?.[1]).toBe('muted')
  })

  it('renders rows as interactive buttons (collapsed by default)', async () => {
    const html = await render([signal()])
    expect(html).toContain('<button')
    expect(html).toContain('aria-expanded="false"')
  })

  it('shows the empty state when there are no signals', async () => {
    const html = await render([])
    expect(html).toContain('No recent learning outcomes')
  })
})
