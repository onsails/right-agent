import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import AsyncState from './AsyncState.vue'
import { resolveAsyncState } from './asyncState'

describe('resolveAsyncState', () => {
  it('prioritises error over everything', () => {
    expect(resolveAsyncState({ loading: true, error: 'boom', empty: true })).toBe('error')
  })
  it('shows loading when no error and still loading', () => {
    expect(resolveAsyncState({ loading: true, error: null, empty: true })).toBe('loading')
  })
  it('shows empty when loaded but empty', () => {
    expect(resolveAsyncState({ loading: false, error: null, empty: true })).toBe('empty')
  })
  it('shows content when loaded and non-empty', () => {
    expect(resolveAsyncState({ loading: false, error: null, empty: false })).toBe('content')
  })
})

describe('AsyncState component', () => {
  async function render(props: Record<string, unknown>) {
    const app = createSSRApp({
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      render: () => h(AsyncState, props as any, () => h('p', 'CONTENT')),
    })
    return renderToString(app)
  }
  it('renders the slot when content state', async () => {
    expect(await render({ loading: false, error: null, empty: false })).toContain('CONTENT')
  })
  it('renders the error text on error', async () => {
    const html = await render({ loading: false, error: 'nope', empty: false })
    expect(html).toContain('nope')
    expect(html).not.toContain('CONTENT')
  })
  it('renders a spinner while loading', async () => {
    const html = await render({ loading: true, error: null, empty: true })
    expect(html).toContain('spinner')
    expect(html).not.toContain('CONTENT')
  })
  it('renders emptyText when empty', async () => {
    const html = await render({ loading: false, error: null, empty: true, emptyText: 'Nothing here' })
    expect(html).toContain('Nothing here')
  })
  it('renders default emptyText when none is provided', async () => {
    const html = await render({ loading: false, error: null, empty: true })
    expect(html).toContain('No data')
  })
})
