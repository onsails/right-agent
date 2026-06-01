import { describe, expect, it, vi } from 'vitest'
import { renderToString } from '@vue/server-renderer'
import { createApp } from 'vue'

// The view calls these on mount; stub them so SSR doesn't hit the network.
vi.mock('../api', () => ({
  providerList: () => Promise.resolve({ providers: [] }),
  providerTypes: () => Promise.resolve({ types: [] }),
  providerCreate: () => Promise.resolve({}),
  providerRotate: () => Promise.resolve({}),
  providerConfigUpdate: () => Promise.resolve({}),
  providerRemove: () => Promise.resolve({}),
}))

import ProvidersView from './ProvidersView.vue'

describe('ProvidersView', () => {
  it('renders the panel without throwing', async () => {
    const html = await renderToString(createApp(ProvidersView))
    expect(html).toContain('Providers')
  })
})
