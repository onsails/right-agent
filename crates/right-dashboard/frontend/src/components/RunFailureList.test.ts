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
    const html = await render({ runs: [failedRun('run-aaaaaaaa'), failedRun('run-bbbbbbbb')] })
    expect(html).toContain('cron')
    expect(html).toContain('job-x')
  })
  it('shows an empty hint when there are no runs', async () => {
    const html = await render({ runs: [] })
    expect(html).toContain('No failures')
  })
})
