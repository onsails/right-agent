import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it, vi } from 'vitest'

import RunFailureList from './RunFailureList.vue'

vi.mock('../api', () => ({ runDetail: vi.fn() }))

function failedRun(id: string) {
  return {
    id,
    kind: 'cron',
    producer_ref: 'job-x',
    status: 'failed',
    started_at: null,
    finished_at: '2026-05-31T11:00:00Z',
    exit_code: 1,
    delivery_required: false,
    delivery_status: 'none',
    delivery_kind: null,
    run_note: null,
    cost_usd: 0.12,
  }
}

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    render: () => h(RunFailureList, props as any),
  })
  return renderToString(app)
}

describe('RunFailureList', () => {
  it('renders one row per failed run', async () => {
    const html = await render({ runs: [failedRun('run-aaaaaaaa'), failedRun('run-bbbbbbbb')], total: 2 })
    expect(html).toContain('cron')
    expect(html).toContain('job-x')
  })
  it('shows an empty hint when there are no runs', async () => {
    const html = await render({ runs: [], total: 0 })
    expect(html).toContain('No failures')
  })
  it('shows a sample label when the total exceeds the shown rows', async () => {
    const runs = Array.from({ length: 50 }, (_, i) => failedRun(`run-${i.toString().padStart(8, '0')}`))
    const html = await render({ runs, total: 137 })
    expect(html).toContain('latest 50 of 137')
  })
  it('omits the sample label when all failures are shown', async () => {
    const html = await render({ runs: [failedRun('run-aaaaaaaa')], total: 1 })
    expect(html).not.toContain('latest')
  })
})
