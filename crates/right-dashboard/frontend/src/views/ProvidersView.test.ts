import { describe, expect, it, vi } from 'vitest'
import { renderToString } from '@vue/server-renderer'
import { createApp, createSSRApp, h } from 'vue'
import type { ProviderProfileView } from '../types'

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
import ProviderTypeList from './ProviderTypeList.vue'

function profile(overrides: Partial<ProviderProfileView> = {}): ProviderProfileView {
  return {
    type: 'gitlab',
    display_name: 'GitLab',
    category: 'sourcecontrol',
    env_var: 'GITLAB_TOKEN',
    ...overrides,
  }
}

async function render(types: ProviderProfileView[]) {
  const app = createSSRApp({ render: () => h(ProviderTypeList, { types }) })
  return renderToString(app)
}

describe('ProvidersView', () => {
  it('renders the panel without throwing', async () => {
    const html = await renderToString(createApp(ProvidersView))
    expect(html).toContain('Providers')
  })
})

// The grouping scaffolding was removed: the backend now hides the built-in
// read-only `github` server-side (covered by the Rust `handle_provider_types`
// test) and returns a FLAT list including `right-github` shown as "GitHub".
// This is the presentational list component; these tests pin the flat rendering
// and the "never surface a raw slug" rule — filtering itself is not its job.
describe('ProvidersView provider-type chooser (flat list)', () => {
  it('renders a flat .type-card per provider type — no grouping wrapper', async () => {
    const html = await render([
      profile({ type: 'right-github', display_name: 'GitHub', env_var: 'GITHUB_TOKEN' }),
      profile({ type: 'gitlab', display_name: 'GitLab', env_var: 'GITLAB_TOKEN' }),
    ])
    // One <article class="type-card"> per type, rendered directly (flat).
    const cards = html.match(/class="type-card"/g) ?? []
    expect(cards).toHaveLength(2)
    // No grouped access-variant markup survives the revert.
    expect(html).not.toContain('access-variant')
  })

  it('surfaces each provider type by its friendly display_name and env var', async () => {
    const html = await render([
      profile({ type: 'right-github', display_name: 'GitHub', env_var: 'GITHUB_TOKEN' }),
      profile({ type: 'gitlab', display_name: 'GitLab', env_var: 'GITLAB_TOKEN' }),
    ])
    // The card label is the human display_name; the env var is shown beneath it.
    expect(html).toContain('>GitHub</strong>')
    expect(html).toContain('>GitLab</strong>')
    expect(html).toContain('GITHUB_TOKEN')
    expect(html).toContain('GITLAB_TOKEN')
  })

  it('renders the friendly display_name, never the raw `right-*` slug', async () => {
    // Guards the "dashboard MUST NOT surface raw slugs" rule: display_name is the
    // visible label, and the distinct `right-github` slug (bound only to :key)
    // must not appear in markup — it would if the template ever rendered `t.type`.
    const html = await render([
      profile({ type: 'right-github', display_name: 'GitHub', env_var: 'GITHUB_TOKEN' }),
    ])
    expect(html).toContain('>GitHub</strong>')
    expect(html).not.toContain('right-github')
  })

  it('renders the empty fallback when no types are available', async () => {
    const html = await render([])
    expect(html).toContain('No provider types available')
    expect(html).not.toContain('class="type-card"')
  })
})
