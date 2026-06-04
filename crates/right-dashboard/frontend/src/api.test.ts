import { afterEach, describe, expect, it, vi } from 'vitest'
import { browserUsageTimezone, usageOverview } from './api'

function usagePayload() {
  return {
    agent: 'right',
    generated_at: '2026-06-04T12:00:00Z',
    timezone: 'Asia/Dubai',
    windows: [],
    selected_window: 'last_30_days',
    daily_series: [],
    source_series: [],
    warnings: [],
  }
}

describe('usageOverview', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it('sends the browser timezone as a query parameter', async () => {
    const fetchMock = vi.fn(async (_path: string, _options?: RequestInit) => new Response(JSON.stringify(usagePayload()), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('window', { Telegram: { WebApp: { initData: 'signed-init' } } })
    vi.spyOn(Intl, 'DateTimeFormat').mockReturnValue({
      resolvedOptions: () => ({ timeZone: 'Asia/Dubai' }),
    } as unknown as Intl.DateTimeFormat)

    await usageOverview()

    expect(fetchMock).toHaveBeenCalledOnce()
    expect(fetchMock.mock.calls[0][0]).toBe('api/v1/usage?timezone=Asia%2FDubai')
  })

  it('falls back to UTC if Intl returns no timezone', () => {
    vi.spyOn(Intl, 'DateTimeFormat').mockReturnValue({
      resolvedOptions: () => ({ timeZone: undefined }),
    } as unknown as Intl.DateTimeFormat)

    expect(browserUsageTimezone()).toBe('UTC')
  })
})
