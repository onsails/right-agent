import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import MetricCard from './MetricCard.vue'

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(MetricCard, props as any),
  })
  return renderToString(app)
}

describe('MetricCard', () => {
  it('renders a static article by default', async () => {
    const html = await render({ label: 'Failures', value: 0, tone: 'default' })
    expect(html).toContain('<article')
    expect(html).not.toContain('<button')
  })
  it('renders a clickable button when interactive', async () => {
    const html = await render({ label: 'Failures', value: 3, tone: 'bad', interactive: true })
    expect(html).toContain('<button')
    expect(html).toContain('type="button"')
    expect(html).toContain('Failures')
    expect(html).toContain('3')
  })
})
