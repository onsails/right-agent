import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import type { LearningEventSummary } from '../types'
import FailedSkillList from './FailedSkillList.vue'

function failedEvent(
  id: number,
  skill: string,
  over: Partial<LearningEventSummary> = {},
): LearningEventSummary {
  return {
    id,
    skill_name: skill,
    action: 'create',
    status: 'failed',
    message: 'boom',
    summary: null,
    created_at: '2026-05-31T11:00:00Z',
    ...over,
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
    const html = await render({ events: [failedEvent(1, 'rightx-a'), failedEvent(2, 'rightx-b')], total: 2 })
    expect(html).toContain('rightx-a')
    expect(html).toContain('rightx-b')
  })

  it('shows an empty hint when there are no events', async () => {
    const html = await render({ events: [], total: 0 })
    expect(html).toContain('No failures')
  })

  it('shows a sample label when the total exceeds the shown rows', async () => {
    const events = Array.from({ length: 50 }, (_, i) => failedEvent(i + 1, `rightx-${i}`))
    const html = await render({ events, total: 137 })
    expect(html).toContain('latest 50 of 137')
  })

  it('omits the sample label when all events are shown', async () => {
    const html = await render({ events: [failedEvent(1, 'rightx-a')], total: 1 })
    expect(html).not.toContain('latest')
  })

  it('keeps duplicate skill and timestamp rows distinct by id', async () => {
    const createdAt = '2026-05-31T11:00:00Z'
    const html = await render({
      events: [
        failedEvent(1, 'rightx-dupe', {
          created_at: createdAt,
          message: 'first failure',
          summary: 'first summary',
        }),
        failedEvent(2, 'rightx-dupe', {
          created_at: createdAt,
          message: 'second failure',
          summary: 'second summary',
        }),
      ],
      total: 2,
    })

    expect(html.match(/<button[^>]*aria-expanded="false"/g)).toHaveLength(2)
    expect(html.match(/rightx-dupe/g)).toHaveLength(2)
    expect(html).toContain('first failure')
    expect(html).toContain('second failure')
  })
})
