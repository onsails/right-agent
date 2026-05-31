import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import TokenLegend from './TokenLegend.vue'

describe('TokenLegend', () => {
  it('labels all four token types', async () => {
    const app = createSSRApp({ render: () => h(TokenLegend) })
    const html = await renderToString(app)
    expect(html).toContain('token-legend')
    expect(html).toContain('input')
    expect(html).toContain('output')
    expect(html).toContain('cache create')
    expect(html).toContain('cache read')
  })
})
