import { afterEach, describe, expect, it, vi } from 'vitest'
import { browserUsageTimezone, focusGet, focusUpdate, providerPeers, usageOverview } from './api'

function usagePayload() {
  return {
    agent: 'right',
    generated_at: '2026-06-04T12:00:00Z',
    timezone: 'Asia/Dubai',
    selected_range: 'last_7_days',
    selected_window: 'last_7_days',
    window: {
      key: 'last_7_days',
      label: 'Last 7 days',
      range_start: '2026-05-29T00:00:00+04:00',
      range_end: '2026-06-04T16:00:00+04:00',
      range_label: 'Asia/Dubai · May 29 00:00-Jun 4 16:00',
      sources: [],
      total_cost_usd: 0,
      subscription_cost_usd: 0,
      api_cost_usd: 0,
      turns: 0,
      invocations: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_creation_tokens: 0,
      cache_read_tokens: 0,
      web_search_requests: 0,
      web_fetch_requests: 0,
      per_model: [],
      budget_skip_count: 0,
    },
    windows: [],
    daily_series: [],
    source_series: [],
    cron_jobs: [],
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
    expect(fetchMock.mock.calls[0][0]).toBe('api/v1/usage?timezone=Asia%2FDubai&range=last_7_days')
  })

  it('sends an explicit usage range when provided', async () => {
    const fetchMock = vi.fn(async (_path: string, _options?: RequestInit) => new Response(JSON.stringify(usagePayload()), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('window', { Telegram: { WebApp: { initData: 'signed-init' } } })

    await usageOverview({ timezone: 'UTC', range: 'last_3_days' })

    expect(fetchMock).toHaveBeenCalledOnce()
    expect(fetchMock.mock.calls[0][0]).toBe('api/v1/usage?timezone=UTC&range=last_3_days')
  })

  it('falls back to UTC if Intl returns no timezone', () => {
    vi.spyOn(Intl, 'DateTimeFormat').mockReturnValue({
      resolvedOptions: () => ({ timeZone: undefined }),
    } as unknown as Intl.DateTimeFormat)

    expect(browserUsageTimezone()).toBe('UTC')
  })
})

describe('focus API', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it('gets conversation focus with chat and thread query parameters', async () => {
    const fetchMock = vi.fn(async (_path: string, _options?: RequestInit) => new Response(JSON.stringify({
      operator_focus: 'existing focus',
    }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('window', { Telegram: { WebApp: { initData: 'signed-init' } } })

    await expect(focusGet(-100123, 7, 'scope-token')).resolves.toEqual({ operator_focus: 'existing focus' })

    expect(fetchMock).toHaveBeenCalledOnce()
    expect(fetchMock.mock.calls[0][0]).toBe('api/v1/focus?chat_id=-100123&thread_id=7&token=scope-token')
    expect((fetchMock.mock.calls[0][1]?.headers as Headers).get('Authorization')).toBe('tma signed-init')
  })

  it('patches conversation focus with chat and thread identifiers', async () => {
    const fetchMock = vi.fn(async (_path: string, _options?: RequestInit) => new Response(JSON.stringify({
      operator_focus: 'focus',
    }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    }))
    vi.stubGlobal('fetch', fetchMock)
    vi.stubGlobal('window', { Telegram: { WebApp: { initData: 'signed-init' } } })

    await expect(focusUpdate(42, 0, 'scope-token', 'focus')).resolves.toEqual({ operator_focus: 'focus' })

    expect(fetchMock).toHaveBeenCalledOnce()
    const [path, options] = fetchMock.mock.calls[0]
    expect(path).toBe('api/v1/focus')
    expect(options?.method).toBe('PATCH')
    expect(JSON.parse(String(options?.body))).toEqual({
      chat_id: 42,
      thread_id: 0,
      token: 'scope-token',
      operator_focus: 'focus',
    })
    expect((options?.headers as Headers).get('Authorization')).toBe('tma signed-init')
  })
})

describe('provider sharing api', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it('providerPeers GETs the peers endpoint', async () => {
    vi.stubGlobal('window', {})
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ peers: [] }), { status: 200 }),
    )
    const res = await providerPeers()
    expect(res.peers).toEqual([])
    const [path, init] = fetchMock.mock.calls[0]
    expect(path).toBe('api/v1/providers/peers')
    expect(init?.method ?? 'GET').toBe('GET')
  })

})
