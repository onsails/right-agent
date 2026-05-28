import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import CollapsibleSection from './CollapsibleSection.vue'

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(CollapsibleSection, props as any, () => h('p', 'BODY')),
  })
  return renderToString(app)
}

describe('CollapsibleSection', () => {
  it('shows the title and count badge', async () => {
    const html = await render({ title: 'core', count: 3 })
    expect(html).toContain('core')
    expect(html).toContain('3')
  })
  it('hides the body when collapsed by default', async () => {
    const html = await render({ title: 'core', count: 3 })
    expect(html).not.toContain('BODY')
  })
  it('shows the body when defaultOpen is true', async () => {
    const html = await render({ title: 'core', count: 3, defaultOpen: true })
    expect(html).toContain('BODY')
  })
})
