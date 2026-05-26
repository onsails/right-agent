import { describe, expect, it } from 'vitest'

import type { McpDetectResponse, McpServerSummary } from '../types'
import {
  canSaveServer,
  createDetectionRequest,
  openOAuthUrl,
  resetAddFlowState,
  seedHeaderRows,
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
