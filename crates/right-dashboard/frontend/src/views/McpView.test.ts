import { describe, expect, it, vi } from 'vitest'
import { renderToString } from '@vue/server-renderer'
import { createApp } from 'vue'

// The view calls these on mount; stub them so SSR doesn't hit the network.
vi.mock('../api', () => ({
  mcpServers: () => Promise.resolve({ servers: [] }),
  mcpDetect: () => Promise.resolve({ bare_url: '', oauth_discovered: false, recommended_mode: 'headers', reason: '', oauth: null }),
  mcpAdd: () => Promise.resolve({}),
  mcpRemove: () => Promise.resolve({}),
  mcpSetHeaders: () => Promise.resolve({}),
  mcpStartOAuth: () => Promise.resolve({ flow_id: '', auth_url: '' }),
  mcpOAuthStatus: () => Promise.resolve({ flow_id: '', server_name: '', status: 'pending', message: null, updated_at: '' }),
  DashboardApiError: class DashboardApiError extends Error { isLocked = false },
}))

import McpView from './McpView.vue'

describe('McpView', () => {
  it('renders the panel without throwing', async () => {
    const html = await renderToString(createApp(McpView))
    expect(html).toContain('Servers')
  })
})

import type { McpDetectResponse, McpServerSummary } from '../types'
import {
  canSaveServer,
  createDetectionRequest,
  evaluateHttpUrlSubmit,
  isOAuthTerminalStatus,
  mcpStatusDetail,
  oauthPollUnavailableStatus,
  oauthStatusMessage,
  openOAuthUrl,
  relativeAgo,
  resetAddFlowState,
  seedHeaderRows,
  shouldApplyOAuthPollResult,
  shouldApplyDetectionResult,
} from './mcpViewModel'

function detection(overrides: Partial<McpDetectResponse> = {}): McpDetectResponse {
  return {
    bare_url: 'https://example.test/mcp',
    oauth_discovered: false,
    recommended_mode: 'headers',
    reason: 'headers required',
    oauth: null,
    ...overrides,
  }
}

describe('McpView model behavior', () => {
  it('disables add save while detection is in flight', () => {
    expect(canSaveServer({
      name: 'github',
      url: 'https://example.test/mcp',
      busyAction: 'detect',
      detectingAddAuth: false,
    })).toBe(false)
    expect(canSaveServer({
      name: 'github',
      url: 'https://example.test/mcp',
      busyAction: 'add',
      detectingAddAuth: false,
    })).toBe(false)
    expect(canSaveServer({
      name: 'github',
      url: 'https://example.test/mcp',
      busyAction: null,
      detectingAddAuth: false,
    })).toBe(true)
  })

  it('keeps add save disabled while detection is in flight and another server action is busy', () => {
    const state = {
      name: 'github',
      url: 'https://example.test/mcp',
      busyAction: 'oauth:existing',
      detectingAddAuth: true,
    }

    expect(canSaveServer(state)).toBe(false)
  })

  it('ignores detection results after the add flow or URL changes', () => {
    const request = createDetectionRequest({ formGeneration: 7, latestRequestId: 3, url: ' https://example.test/mcp ' })

    expect(request).not.toBeNull()
    expect(shouldApplyDetectionResult(request!, {
      formGeneration: 7,
      latestRequestId: request!.requestId,
      url: 'https://example.test/mcp',
    })).toBe(true)
    expect(shouldApplyDetectionResult(request!, {
      formGeneration: 8,
      latestRequestId: 3,
      url: 'https://example.test/mcp',
    })).toBe(false)
    expect(shouldApplyDetectionResult(request!, {
      formGeneration: 7,
      latestRequestId: request!.requestId + 1,
      url: 'https://example.test/mcp',
    })).toBe(false)
    expect(shouldApplyDetectionResult(request!, {
      formGeneration: 7,
      latestRequestId: 3,
      url: 'https://changed.test/mcp',
    })).toBe(false)
  })

  it('invalidates pending detection when the add flow resets', () => {
    const request = createDetectionRequest({ formGeneration: 4, latestRequestId: 8, url: 'https://example.test/mcp' })!
    const reset = resetAddFlowState({
      formGeneration: request.formGeneration,
      latestRequestId: request.requestId,
      activeDetectionRequestId: request.requestId,
    })

    expect(reset.activeDetectionRequestId).toBeNull()
    expect(shouldApplyDetectionResult(request, {
      formGeneration: reset.formGeneration,
      latestRequestId: reset.latestRequestId,
      url: request.url,
    })).toBe(false)
  })

  it('keeps saved header values write-only when editing headers', () => {
    const server: McpServerSummary = {
      name: 'github',
      url: 'https://example.test/mcp',
      status: 'needs_auth',
      tool_count: 0,
      auth_type: 'headers',
      header_names: ['Authorization', 'X-Api-Key'],
      protected: false,
    }

    expect(seedHeaderRows(server)).toEqual([
      { name: 'Authorization', value: '' },
      { name: 'X-Api-Key', value: '' },
    ])
  })

  it('defaults header editing to an empty Authorization row', () => {
    expect(seedHeaderRows({ ...serverStub(), header_names: [] })).toEqual([{ name: 'Authorization', value: '' }])
  })

  it('opens OAuth auth URLs through Telegram before falling back to location assignment', () => {
    const opened: string[] = []
    const assigned: string[] = []

    openOAuthUrl('https://auth.test', {
      openTelegramLink: (url) => opened.push(url),
      assignLocation: (url) => assigned.push(url),
    })

    expect(opened).toEqual(['https://auth.test'])
    expect(assigned).toEqual([])

    openOAuthUrl('https://fallback.test', {
      assignLocation: (url) => assigned.push(url),
    })

    expect(assigned).toEqual(['https://fallback.test'])
  })

  it('does not create a detection request for a blank URL', () => {
    expect(createDetectionRequest({ formGeneration: 0, latestRequestId: 0, url: '   ' })).toBeNull()
  })

  it('uses the response recommendation only when a detection result is current', () => {
    const request = createDetectionRequest({ formGeneration: 1, latestRequestId: 9, url: 'https://example.test/mcp' })
    const response = detection({ recommended_mode: 'oauth' })

    expect(shouldApplyDetectionResult(request!, {
      formGeneration: 1,
      latestRequestId: request!.requestId,
      url: response.bare_url,
    })).toBe(true)
  })

  it('treats only non-pending OAuth statuses as terminal', () => {
    expect(isOAuthTerminalStatus('pending')).toBe(false)
    expect(isOAuthTerminalStatus('succeeded')).toBe(true)
    expect(isOAuthTerminalStatus('failed')).toBe(true)
    expect(isOAuthTerminalStatus('expired')).toBe(true)
    expect(isOAuthTerminalStatus('unknown')).toBe(true)
  })

  it('ignores stale OAuth poll results after a newer flow starts', () => {
    expect(shouldApplyOAuthPollResult('flow-new', 'flow-new')).toBe(true)
    expect(shouldApplyOAuthPollResult('flow-old', 'flow-new')).toBe(false)
    expect(shouldApplyOAuthPollResult('flow-old', undefined)).toBe(false)
  })

  it('formats OAuth status messages without relying on Telegram', () => {
    expect(oauthStatusMessage({ flow_id: 'f1', server_name: 'composio', status: 'pending', message: null, updated_at: 'now' })).toBe('OAuth pending')
    expect(oauthStatusMessage({ flow_id: 'f1', server_name: 'composio', status: 'succeeded', message: null, updated_at: 'now' })).toBe('OAuth connected')
    expect(oauthStatusMessage({ flow_id: 'f1', server_name: 'composio', status: 'failed', message: 'MCP readiness failed', updated_at: 'now' })).toBe('MCP readiness failed')
  })

  it('keeps transient OAuth status poll failures non-terminal', () => {
    const status = oauthPollUnavailableStatus('f1', 'composio', new Error('Network unavailable'))

    expect(status).toMatchObject({
      flow_id: 'f1',
      server_name: 'composio',
      status: 'pending',
      message: 'OAuth status unavailable; retrying: Network unavailable',
    })
    expect(isOAuthTerminalStatus(status.status)).toBe(false)
  })
})

describe('mcpStatusDetail', () => {
  const now = new Date('2026-06-01T12:00:30Z')

  it('returns null for connected servers', () => {
    expect(
      mcpStatusDetail(
        { status: 'connected', last_connect_error: 'x', last_attempt_at: '2026-06-01T12:00:00Z' },
        now,
      ),
    ).toBeNull()
  })

  it('combines error and last-tried for unreachable', () => {
    expect(
      mcpStatusDetail(
        { status: 'unreachable', last_connect_error: 'connection refused', last_attempt_at: '2026-06-01T12:00:18Z' },
        now,
      ),
    ).toBe('connection refused · last tried 12s ago')
  })

  it('returns null when no cause recorded', () => {
    expect(mcpStatusDetail({ status: 'unreachable' }, now)).toBeNull()
  })
})

describe('relativeAgo', () => {
  const now = new Date('2026-06-01T12:00:30Z')
  it('formats seconds and minutes', () => {
    expect(relativeAgo('2026-06-01T12:00:18Z', now)).toBe('12s ago')
    expect(relativeAgo('2026-06-01T11:58:30Z', now)).toBe('2m ago')
  })
  it('returns null for unparseable input', () => {
    expect(relativeAgo('not-a-date', now)).toBeNull()
  })
})

describe('evaluateHttpUrlSubmit', () => {
  it('blocks the first submit of a plaintext http:// url', () => {
    const r = evaluateHttpUrlSubmit('http://openclaw.owl-skate.ts.net:27123/mcp', false)
    expect(r.proceed).toBe(false)
    expect(r.warning).toContain('without TLS')
  })

  it('proceeds on the second submit (already warned)', () => {
    expect(evaluateHttpUrlSubmit('http://box.local/mcp', true)).toEqual({ proceed: true, warning: null })
  })

  it('never warns for https:// urls', () => {
    expect(evaluateHttpUrlSubmit('https://mcp.example.com/mcp', false)).toEqual({ proceed: true, warning: null })
  })
})

function serverStub(): McpServerSummary {
  return {
    name: 'server',
    url: 'https://example.test/mcp',
    status: 'connected',
    tool_count: 1,
    auth_type: 'headers',
    header_names: [],
    protected: false,
  }
}
