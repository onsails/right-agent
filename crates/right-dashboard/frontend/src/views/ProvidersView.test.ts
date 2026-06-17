// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { renderToString } from '@vue/server-renderer'
import { createApp, createSSRApp, h, nextTick } from 'vue'
import type { App } from 'vue'
import type { ProviderProfileView, ProviderView } from '../types'
import { HOSTS_MICROCOPY } from './providersViewModel'

// The view calls these on mount; stub them so SSR doesn't hit the network.
const apiMocks = vi.hoisted(() => ({
  providerList: vi.fn(),
  providerTypes: vi.fn(),
  providerCreate: vi.fn(),
  providerRotate: vi.fn(),
  providerConfigUpdate: vi.fn(),
  providerRemove: vi.fn(),
  providerPeers: vi.fn(),
  providerShare: vi.fn(),
  providerUnshare: vi.fn(),
  providerBorrow: vi.fn(),
}))

vi.mock('../api', () => apiMocks)

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

function provider(overrides: Partial<ProviderView> = {}): ProviderView {
  return {
    name: 'fal',
    type: 'generic',
    label: 'fal',
    env_var: 'FAL_KEY',
    generic: {
      env_var: 'FAL_KEY',
      upstream_hosts: ['fal.run', 'queue.fal.run'],
      upstream_path_prefix: '/v1',
    },
    updated_at: null,
    composed: true,
    status: { kind: 'healthy' },
    ...overrides,
  }
}

function mountProvidersView(): { app: App<Element>; root: HTMLElement } {
  const root = document.createElement('div')
  document.body.append(root)
  const app = createApp(ProvidersView)
  app.mount(root)
  return { app, root }
}

async function flushAsync(): Promise<void> {
  await nextTick()
  await Promise.resolve()
  await nextTick()
}

function buttonsByText(root: ParentNode, text: string): HTMLButtonElement[] {
  return Array
    .from(root.querySelectorAll<HTMLButtonElement>('button'))
    .filter((button) => button.textContent?.trim() === text)
}

function hostButtonsByText(root: ParentNode, text: string): HTMLButtonElement[] {
  return Array
    .from(root.querySelectorAll<HTMLButtonElement>('.hosts-list .host-row button'))
    .filter((button) => button.textContent?.trim() === text)
}

function clickButton(root: ParentNode, text: string, index = 0): void {
  const button = buttonsByText(root, text)[index]
  expect(button).toBeDefined()
  button.click()
}

function inputByPlaceholder(root: ParentNode, placeholder: string): HTMLInputElement {
  const input = root.querySelector<HTMLInputElement>(`input[placeholder="${placeholder}"]`)
  expect(input).not.toBeNull()
  return input!
}

function inputByAriaLabel(root: ParentNode, ariaLabel: string): HTMLInputElement {
  const input = root.querySelector<HTMLInputElement>(`input[aria-label="${ariaLabel}"]`)
  expect(input).not.toBeNull()
  return input!
}

function setInput(input: HTMLInputElement, value: string): void {
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
}

beforeEach(() => {
  apiMocks.providerList.mockResolvedValue({ providers: [] })
  apiMocks.providerTypes.mockResolvedValue({ types: [] })
  apiMocks.providerCreate.mockResolvedValue({})
  apiMocks.providerRotate.mockResolvedValue({})
  apiMocks.providerConfigUpdate.mockResolvedValue({})
  apiMocks.providerRemove.mockResolvedValue({})
  apiMocks.providerPeers.mockResolvedValue({ peers: [] })
  apiMocks.providerShare.mockResolvedValue({})
  apiMocks.providerUnshare.mockResolvedValue({})
  apiMocks.providerBorrow.mockResolvedValue({})
})

afterEach(() => {
  vi.clearAllMocks()
  document.body.innerHTML = ''
})

describe('ProvidersView', () => {
  it('renders the panel without throwing', async () => {
    const html = await renderToString(createApp(ProvidersView))
    expect(html).toContain('Providers')
    expect(html).not.toContain('Header name')
  })

  it('defines generic hosts copy without the old header-name field', () => {
    expect(HOSTS_MICROCOPY).toContain('$ENV_VAR')
    expect(HOSTS_MICROCOPY).toContain('Right stores')
    expect(HOSTS_MICROCOPY).not.toContain('Header name')
  })

  it('creates generic providers from env-var and multi-host fields only', async () => {
    apiMocks.providerTypes.mockResolvedValue({
      types: [profile({
        type: 'generic',
        display_name: 'Generic',
        env_var: 'GENERIC_API_KEY',
      })],
    })

    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      clickButton(root, '+ Add')
      await flushAsync()

      const typeCard = Array
        .from(root.querySelectorAll<HTMLElement>('.type-card'))
        .find((card) => card.textContent?.includes('Generic'))
      expect(typeCard).toBeDefined()
      typeCard!.click()
      await flushAsync()

      expect(root.textContent).toContain('Upstream hosts')
      expect(root.textContent).toContain(HOSTS_MICROCOPY)
      expect(root.textContent).not.toContain('Header name')

      setInput(inputByPlaceholder(root, 'e.g. my-openai'), 'fal')
      setInput(inputByPlaceholder(root, 'e.g. OPENAI_API_KEY'), 'FAL_KEY')
      setInput(inputByAriaLabel(root, 'Upstream host 1'), ' fal.run ')
      clickButton(root, 'Add')
      await flushAsync()
      setInput(inputByAriaLabel(root, 'Upstream host 2'), ' queue.fal.run ')
      clickButton(root, 'Add')
      await flushAsync()
      hostButtonsByText(root, 'Remove').at(-1)!.click()
      setInput(inputByPlaceholder(root, 'Paste API key'), 'secret-fal-key')
      await flushAsync()

      clickButton(root, 'Save')
      await flushAsync()

      expect(apiMocks.providerCreate).toHaveBeenCalledTimes(1)
      const request = apiMocks.providerCreate.mock.calls[0][0]
      expect(request).toEqual({
        type: 'generic',
        label: 'fal',
        credential: 'secret-fal-key',
        generic: {
          env_var: 'FAL_KEY',
          upstream_hosts: ['fal.run', 'queue.fal.run'],
          upstream_path_prefix: undefined,
        },
      })
      expect(request.generic).not.toHaveProperty('header_name')
      expect(request.generic).not.toHaveProperty('upstream_host')
    } finally {
      app.unmount()
    }
  })

  it('updates generic providers with normalized upstream_hosts and no legacy host fields', async () => {
    apiMocks.providerList.mockResolvedValue({ providers: [provider()] })
    apiMocks.providerTypes.mockResolvedValue({
      types: [profile({
        type: 'generic',
        display_name: 'Generic',
        env_var: 'GENERIC_API_KEY',
      })],
    })

    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      clickButton(root, 'Edit')
      await flushAsync()

      expect(root.textContent).toContain('Upstream hosts')
      expect(root.textContent).not.toContain('Header name')
      expect(inputByAriaLabel(root, 'Upstream host 1').value).toBe('fal.run')
      expect(inputByAriaLabel(root, 'Upstream host 2').value).toBe('queue.fal.run')

      hostButtonsByText(root, 'Remove').at(-1)!.click()
      await flushAsync()
      setInput(inputByAriaLabel(root, 'Upstream host 1'), ' rest.fal.ai ')
      await flushAsync()

      clickButton(root, 'Save')
      await flushAsync()

      expect(apiMocks.providerConfigUpdate).toHaveBeenCalledTimes(1)
      const [name, request] = apiMocks.providerConfigUpdate.mock.calls[0]
      expect(name).toBe('fal')
      expect(request).toEqual({
        env_var: 'FAL_KEY',
        upstream_hosts: ['rest.fal.ai'],
        upstream_path_prefix: '/v1',
      })
      expect(request).not.toHaveProperty('header_name')
      expect(request).not.toHaveProperty('upstream_host')
    } finally {
      app.unmount()
    }
  })

  it('keeps the providers list when peer discovery fails', async () => {
    apiMocks.providerList.mockResolvedValue({ providers: [provider()] })
    apiMocks.providerPeers.mockRejectedValue(new Error('peers boom'))

    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      // The primary list still renders despite the peer-discovery failure;
      // the failure is not surfaced as a fatal view error.
      expect(buttonsByText(root, 'Share with…').length).toBe(1)
      expect(root.textContent).not.toContain('peers boom')
    } finally {
      app.unmount()
    }
  })

  it('shares an owned provider with a peer and refreshes peer state', async () => {
    apiMocks.providerList.mockResolvedValue({ providers: [provider()] })
    apiMocks.providerPeers.mockResolvedValue({
      peers: [{ agent: 'beta', network_policy: 'permissive', providers: [] }],
    })

    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      const peersCallsAtMount = apiMocks.providerPeers.mock.calls.length
      clickButton(root, 'Share with…') // open the share modal for the single row
      await flushAsync()

      const betaRow = Array
        .from(root.querySelectorAll<HTMLElement>('article'))
        .find((el) => el.textContent?.includes('beta'))
      expect(betaRow).toBeDefined()
      betaRow!.querySelector('button')!.click()
      await flushAsync()
      await flushAsync()

      expect(apiMocks.providerShare).toHaveBeenCalledTimes(1)
      expect(apiMocks.providerShare.mock.calls[0][0]).toEqual({ provider: 'fal', dest_agent: 'beta' })
      // A successful share refreshes peers so the target now reports 'already shared'.
      expect(apiMocks.providerPeers.mock.calls.length).toBe(peersCallsAtMount + 1)
    } finally {
      app.unmount()
    }
  })

  it('renders a per-row Share entry point for owned providers', async () => {
    apiMocks.providerList.mockResolvedValue({ providers: [provider()] })
    const { app, root } = mountProvidersView()
    await flushAsync()

    // Per-row Share button (one owned provider row); the old Import/Export UI is gone.
    expect(buttonsByText(root, 'Share with…').length).toBe(1)
    expect(buttonsByText(root, 'Import').length).toBe(0)
    expect(buttonsByText(root, 'Export').length).toBe(0)

    app.unmount()
  })

  it('renders a borrowed provider read-only: shared-from label + Unshare, no owner actions', async () => {
    apiMocks.providerList.mockResolvedValue({
      providers: [provider({ name: 'borrowed-fal', shared_from: 'riskoff' })],
    })
    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      expect(root.textContent).toContain('Shared from riskoff')
      expect(buttonsByText(root, 'Unshare').length).toBe(1)
      // The owner-only actions must not render for a borrowed provider.
      expect(buttonsByText(root, 'Rotate').length).toBe(0)
      expect(buttonsByText(root, 'Edit').length).toBe(0)
      expect(buttonsByText(root, 'Remove').length).toBe(0)
      expect(buttonsByText(root, 'Share with…').length).toBe(0)
    } finally {
      app.unmount()
    }
  })

  it('unshares a borrowed provider and refreshes', async () => {
    apiMocks.providerList.mockResolvedValue({
      providers: [provider({ name: 'borrowed-fal', shared_from: 'riskoff' })],
    })
    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      const listCallsAtMount = apiMocks.providerList.mock.calls.length
      clickButton(root, 'Unshare')
      await flushAsync()
      await flushAsync()

      expect(apiMocks.providerUnshare).toHaveBeenCalledTimes(1)
      expect(apiMocks.providerUnshare.mock.calls[0][0]).toEqual({ provider: 'borrowed-fal' })
      expect(apiMocks.providerList.mock.calls.length).toBe(listCallsAtMount + 1)
    } finally {
      app.unmount()
    }
  })

  it('borrows a provider a peer shares and refreshes the list', async () => {
    apiMocks.providerList.mockResolvedValue({ providers: [] })
    apiMocks.providerPeers.mockResolvedValue({
      peers: [{
        agent: 'riskoff',
        network_policy: 'permissive',
        providers: [{ name: 'fal', type: 'right-fal', env_var: 'FAL_KEY', label: null, generic: null }],
      }],
    })

    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      const listCallsAtMount = apiMocks.providerList.mock.calls.length
      clickButton(root, 'Borrow…') // open the borrow section from the header
      await flushAsync()
      expect(root.textContent).toContain('from riskoff')

      clickButton(root, 'Borrow') // the per-candidate borrow button
      await flushAsync()
      await flushAsync()

      expect(apiMocks.providerBorrow).toHaveBeenCalledTimes(1)
      expect(apiMocks.providerBorrow.mock.calls[0][0]).toEqual({ owner_agent: 'riskoff', provider: 'fal' })
      // A successful borrow refreshes the provider list (and peers).
      expect(apiMocks.providerList.mock.calls.length).toBe(listCallsAtMount + 1)
    } finally {
      app.unmount()
    }
  })

  it('blocks borrowing a provider name the current agent already holds', async () => {
    apiMocks.providerList.mockResolvedValue({ providers: [provider({ name: 'fal' })] })
    apiMocks.providerPeers.mockResolvedValue({
      peers: [{
        agent: 'riskoff',
        network_policy: 'permissive',
        providers: [{ name: 'fal', type: 'right-fal', env_var: 'FAL_KEY', label: null, generic: null }],
      }],
    })

    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      clickButton(root, 'Borrow…')
      await flushAsync()

      expect(root.textContent).toContain('already in this agent')
      const borrowBtn = buttonsByText(root, 'Borrow')[0]
      expect(borrowBtn).toBeDefined()
      expect(borrowBtn.disabled).toBe(true)
    } finally {
      app.unmount()
    }
  })

  it('pre-fills generic re-create forms with the prior upstream_hosts', async () => {
    apiMocks.providerList.mockResolvedValue({
      providers: [provider({
        status: { kind: 'missing' },
        generic: {
          env_var: 'FAL_KEY',
          upstream_hosts: ['fal.run', 'queue.fal.run', 'rest.fal.ai'],
          upstream_path_prefix: '/v2',
        },
      })],
    })
    apiMocks.providerTypes.mockResolvedValue({
      types: [profile({
        type: 'generic',
        display_name: 'Generic',
        env_var: 'GENERIC_API_KEY',
      })],
    })

    const { app, root } = mountProvidersView()
    await flushAsync()

    try {
      clickButton(root, 'Re-create')
      await flushAsync()

      expect(inputByPlaceholder(root, 'e.g. my-openai').value).toBe('fal')
      expect(inputByPlaceholder(root, 'e.g. OPENAI_API_KEY').value).toBe('FAL_KEY')
      expect(inputByAriaLabel(root, 'Upstream host 1').value).toBe('fal.run')
      expect(inputByAriaLabel(root, 'Upstream host 2').value).toBe('queue.fal.run')
      expect(inputByAriaLabel(root, 'Upstream host 3').value).toBe('rest.fal.ai')
      expect(inputByPlaceholder(root, 'e.g. /v1').value).toBe('/v2')
    } finally {
      app.unmount()
    }
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
      profile({ type: 'right-fal', display_name: 'fal.ai', env_var: 'FAL_KEY' }),
    ])
    // The card label is the human display_name; the env var is shown beneath it.
    expect(html).toContain('>GitHub</strong>')
    expect(html).toContain('>GitLab</strong>')
    expect(html).toContain('>fal.ai</strong>')
    expect(html).toContain('GITHUB_TOKEN')
    expect(html).toContain('GITLAB_TOKEN')
    expect(html).toContain('FAL_KEY')
    expect(html).not.toContain('right-fal')
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
