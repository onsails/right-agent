import { describe, expect, it } from 'vitest'

import type { CronCard } from '../types'
import { sortCrons } from './cronSort'

function card(name: string, s24: number, s7: number): CronCard {
  return {
    job_name: name,
    schedule: '0 8 * * *',
    schedule_human: 'daily',
    recurring: true,
    run_at: null,
    next_run_at: null,
    target_chat_id: null,
    target_thread_id: null,
    max_budget_usd: 1,
    spend_24h_usd: s24,
    spend_7d_usd: s7,
    last_run: null,
    recent_runs: [],
  }
}

describe('sortCrons', () => {
  const crons = [card('beta', 1, 5), card('alpha', 3, 2), card('gamma', 2, 9)]

  it('sorts by name ascending by default', () => {
    expect(sortCrons(crons, 'name').map((c) => c.job_name)).toEqual(['alpha', 'beta', 'gamma'])
  })

  it('sorts by 24h spend descending, name tie-break', () => {
    expect(sortCrons(crons, 'spend_24h').map((c) => c.job_name)).toEqual(['alpha', 'gamma', 'beta'])
  })

  it('sorts by 7d spend descending', () => {
    expect(sortCrons(crons, 'spend_7d').map((c) => c.job_name)).toEqual(['gamma', 'beta', 'alpha'])
  })

  it('does not mutate the input array', () => {
    const input = [card('b', 0, 0), card('a', 0, 0)]
    sortCrons(input, 'name')
    expect(input.map((c) => c.job_name)).toEqual(['b', 'a'])
  })
})
