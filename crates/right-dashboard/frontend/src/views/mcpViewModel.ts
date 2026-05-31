import type { McpHeaderInput, McpOAuthFlowStatus, McpOAuthStatusResponse, McpServerSummary } from '../types'

export interface SaveServerState {
  name: string
  url: string
  busyAction: string | null
  detectingAddAuth: boolean
}

export interface DetectionRequestInput {
  formGeneration: number
  latestRequestId: number
  url: string
}

export interface DetectionRequest {
  formGeneration: number
  requestId: number
  url: string
}

export interface CurrentDetectionState {
  formGeneration: number
  latestRequestId: number
  url: string
}

export interface AddFlowDetectionState {
  formGeneration: number
  latestRequestId: number
  activeDetectionRequestId: number | null
}

export interface OAuthOpenTarget {
  openTelegramLink?: (url: string) => void
  assignLocation: (url: string) => void
}

export function canSaveServer(state: SaveServerState): boolean {
  return state.name.trim().length > 0
    && state.url.trim().length > 0
    && state.busyAction !== 'add'
    && state.busyAction !== 'detect'
    && !state.detectingAddAuth
}

export function createDetectionRequest(input: DetectionRequestInput): DetectionRequest | null {
  const requestUrl = input.url.trim()
  if (requestUrl.length === 0) {
    return null
  }

  return {
    formGeneration: input.formGeneration,
    requestId: input.latestRequestId + 1,
    url: requestUrl,
  }
}

export function shouldApplyDetectionResult(request: DetectionRequest, current: CurrentDetectionState): boolean {
  return request.formGeneration === current.formGeneration
    && request.requestId === current.latestRequestId
    && request.url === current.url.trim()
}

export function resetAddFlowState(state: AddFlowDetectionState): AddFlowDetectionState {
  return {
    formGeneration: state.formGeneration + 1,
    latestRequestId: state.latestRequestId + 1,
    activeDetectionRequestId: null,
  }
}

export function nonEmptyHeaders(rows: McpHeaderInput[]): McpHeaderInput[] {
  return rows
    .map((header) => ({ name: header.name.trim(), value: header.value }))
    .filter((header) => header.name.length > 0 && header.value.length > 0)
}

export function seedHeaderRows(server: McpServerSummary): McpHeaderInput[] {
  if (server.header_names.length === 0) {
    return [{ name: 'Authorization', value: '' }]
  }
  return server.header_names.map((headerName) => ({ name: headerName, value: '' }))
}

export function openOAuthUrl(authUrl: string, target: OAuthOpenTarget): void {
  if (target.openTelegramLink) {
    target.openTelegramLink(authUrl)
    return
  }
  target.assignLocation(authUrl)
}

export function isOAuthTerminalStatus(status: McpOAuthFlowStatus): boolean {
  return status !== 'pending'
}

export function shouldApplyOAuthPollResult(responseFlowId: string, currentFlowId: string | undefined): boolean {
  return currentFlowId !== undefined && responseFlowId === currentFlowId
}

export function oauthPollUnavailableStatus(flowId: string, serverName: string, err: unknown): McpOAuthStatusResponse {
  const detail = err instanceof Error ? err.message : 'OAuth status unavailable'
  return {
    flow_id: flowId,
    server_name: serverName,
    status: 'pending',
    message: `OAuth status unavailable; retrying: ${detail}`,
    updated_at: new Date().toISOString(),
  }
}

export interface McpStatusDetailInput {
  status: string
  last_connect_error?: string | null
  last_attempt_at?: string | null
}

export function relativeAgo(isoTimestamp: string, now: Date): string | null {
  const then = Date.parse(isoTimestamp)
  if (Number.isNaN(then)) {
    return null
  }
  const seconds = Math.max(0, Math.round((now.getTime() - then) / 1000))
  if (seconds < 60) {
    return `${seconds}s ago`
  }
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) {
    return `${minutes}m ago`
  }
  const hours = Math.round(minutes / 60)
  return `${hours}h ago`
}

export function mcpStatusDetail(server: McpStatusDetailInput, now: Date): string | null {
  if (server.status.toLowerCase() === 'connected') {
    return null
  }
  const parts: string[] = []
  if (server.last_connect_error) {
    parts.push(server.last_connect_error)
  }
  if (server.last_attempt_at) {
    const ago = relativeAgo(server.last_attempt_at, now)
    if (ago) {
      parts.push(`last tried ${ago}`)
    }
  }
  return parts.length > 0 ? parts.join(' · ') : null
}

export function oauthStatusMessage(status: McpOAuthStatusResponse): string {
  if (status.message) {
    return status.message
  }
  if (status.status === 'pending') {
    return 'OAuth pending'
  }
  if (status.status === 'succeeded') {
    return 'OAuth connected'
  }
  if (status.status === 'expired') {
    return 'OAuth flow expired'
  }
  if (status.status === 'unknown') {
    return 'OAuth flow is no longer active'
  }
  return 'OAuth failed'
}
