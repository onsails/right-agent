import { describe, expect, it } from 'vitest'
import { curatorRunStatusTone, curatorRunHeadline } from './curatorRuns'

describe('curatorRunStatusTone', () => {
  it('maps proposed to info', () => { expect(curatorRunStatusTone('proposed')).toBe('info') })
  it('maps failed to bad', () => { expect(curatorRunStatusTone('failed')).toBe('bad') })
  it('maps success to ok', () => { expect(curatorRunStatusTone('success')).toBe('ok') })
})

describe('curatorRunHeadline', () => {
  it('summarises an apply run', () => {
    expect(curatorRunHeadline({ run_at: '2026-06-15T00:00:00Z', trigger: 'time_fallback', mode: 'apply', status: 'success', cost_usd: 0.5, consolidations: 1, archives: 2, summary: null })).toBe('merged 1, archived 2')
  })
  it('labels a report-only run as proposed', () => {
    expect(curatorRunHeadline({ run_at: '2026-06-15T00:00:00Z', trigger: 'cost_spike', mode: 'report_only', status: 'proposed', cost_usd: 0.1, consolidations: 0, archives: 0, summary: '3 proposals' })).toBe('3 proposals')
  })
})
