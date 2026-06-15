import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'
import CuratorRunsPanel from './CuratorRunsPanel.vue'

describe('CuratorRunsPanel', () => {
  it('renders the empty state when there are no runs', async () => {
    const app = createSSRApp({ render: () => h(CuratorRunsPanel, { runs: [], consolidations: [] }) })
    expect(await renderToString(app)).toContain('No curator runs yet')
  })
  it('renders a run headline and a lineage arrow', async () => {
    const app = createSSRApp({ render: () => h(CuratorRunsPanel, {
      runs: [{ run_at: '2026-06-15T00:00:00Z', trigger: 'time_fallback', mode: 'apply', status: 'success', cost_usd: 0.5, consolidations: 1, archives: 2, summary: null }],
      consolidations: [{ absorbed: 'rightx-a', umbrella: 'rightx-umbrella' }],
    }) })
    const html = await renderToString(app)
    expect(html).toContain('merged 1, archived 2')
    expect(html).toContain('rightx-a → rightx-umbrella')
  })
})
