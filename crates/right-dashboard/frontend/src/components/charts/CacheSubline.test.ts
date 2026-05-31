import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import CacheSubline from './CacheSubline.vue'
import type { CacheTokens } from './usageCache'

async function render(tokens: CacheTokens) {
  const app = createSSRApp({
    render: () => h(CacheSubline, { tokens }),
  })
  return renderToString(app)
}

describe('CacheSubline', () => {
  it('renders created/read/hit when there are cache reads', async () => {
    const html = await render({ input_tokens: 10, cache_creation_tokens: 50, cache_read_tokens: 300 })
    expect(html).toContain('created')
    expect(html).toContain('read')
    expect(html).toContain('83%')
    expect(html).toContain('hit')
  })
  it('renders nothing when there are no cache reads', async () => {
    const html = await render({ input_tokens: 10, cache_creation_tokens: 0, cache_read_tokens: 0 })
    expect(html).not.toContain('created')
    expect(html).not.toContain('read')
    expect(html).not.toContain('hit')
  })
})
