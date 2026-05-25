import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, test } from 'vitest'

import AppShell from './AppShell.vue'

describe('AppShell', () => {
  test('uses saved display preference for the display mode toggle label', async () => {
    const app = createSSRApp({
      render() {
        return h(AppShell, {
          agent: 'Agent',
          connectionState: 'live',
          message: 'Live',
          lastUpdatedAt: null,
          tabs: [],
          activeTab: 'overview',
          displayMode: 'normal',
          preferredDisplayMode: 'fullscreen',
        }, () => h('section', 'Dashboard content'))
      },
    })

    const html = await renderToString(app)

    expect(html).toContain('aria-label="Use normal view"')
    expect(html).toContain('>Normal</button>')
  })
})
