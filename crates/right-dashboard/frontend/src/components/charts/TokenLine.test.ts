import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import TokenLine from './TokenLine.vue'
import type { TokenCounts } from './tokenBar'

function counts(over: Partial<TokenCounts> = {}): TokenCounts {
  return { input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0, cache_read_tokens: 0, ...over }
}

async function render(tokens: TokenCounts, compact = false) {
  const app = createSSRApp({ render: () => h(TokenLine, { tokens, compact }) })
  return renderToString(app)
}

describe('TokenLine', () => {
  it('renders all four token counts and the hit-rate bar', async () => {
    const html = await render(
      counts({ input_tokens: 10, output_tokens: 20, cache_creation_tokens: 50, cache_read_tokens: 300 }),
    )
    expect(html).toContain('10')
    expect(html).toContain('20')
    expect(html).toContain('50')
    expect(html).toContain('300')
    expect(html).toContain('hit-bar')
    expect(html).toContain('83%')
  })

  it('omits the hit bar when there are no input-bearing tokens', async () => {
    const html = await render(counts({ output_tokens: 20 }))
    expect(html).toContain('token-line')
    expect(html).not.toContain('hit-bar')
  })

  it('renders the compact layout class when compact', async () => {
    const html = await render(counts({ input_tokens: 10, cache_read_tokens: 90 }), true)
    expect(html).toContain('compact')
  })
})
