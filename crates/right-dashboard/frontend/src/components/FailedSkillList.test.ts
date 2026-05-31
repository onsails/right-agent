import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import FailedSkillList from './FailedSkillList.vue'

function failedEvent(skill: string) {
  return {
    skill_name: skill,
    action: 'create',
    status: 'failed',
    message: 'boom',
    summary: null,
    created_at: '2026-05-31T11:00:00Z',
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(FailedSkillList, props as any),
  })
  return renderToString(app)
}

describe('FailedSkillList', () => {
  it('renders one row per failed event', async () => {
    const html = await render({ events: [failedEvent('rightx-a'), failedEvent('rightx-b')], total: 2 })
    expect(html).toContain('rightx-a')
    expect(html).toContain('rightx-b')
  })
  it('shows an empty hint when there are no events', async () => {
    const html = await render({ events: [], total: 0 })
    expect(html).toContain('No failures')
  })
  it('shows a sample label when the total exceeds the shown rows', async () => {
    const events = Array.from({ length: 50 }, (_, i) => failedEvent(`rightx-${i}`))
    const html = await render({ events, total: 137 })
    expect(html).toContain('latest 50 of 137')
  })
  it('omits the sample label when all events are shown', async () => {
    const html = await render({ events: [failedEvent('rightx-a')], total: 1 })
    expect(html).not.toContain('latest')
  })
})
