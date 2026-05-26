import type {
  ApiErrorBody,
  DashboardOverviewResponse,
  DoctorResponse,
  IdentityFileResponse,
  BootstrapResponse,
  IdentityResponse,
  LearningOverviewResponse,
  McpAddRequest,
  McpDetectRequest,
  McpDetectResponse,
  McpHeaderInput,
  McpHeadersRequest,
  McpMutationResponse,
  McpOAuthStartResponse,
  McpServersResponse,
  OverviewResponse,
  PinSkillRequest,
  PinSkillResponse,
  RunDetailResponse,
  SkillDetailResponse,
  SkillsResponse,
  SandboxStatsResponse,
  UsageOverviewResponse,
} from './types'

export class DashboardApiError extends Error {
  readonly status: number
  readonly code?: string

  constructor(message: string, status: number, code?: string) {
    super(message)
    this.name = 'DashboardApiError'
    this.status = status
    this.code = code
  }

  get isLocked(): boolean {
    return this.status === 401 || this.status === 403
  }
}

export function bootstrap(): Promise<BootstrapResponse> {
  return requestJson<BootstrapResponse>('api/v1/bootstrap')
}

export function dashboardOverview(): Promise<DashboardOverviewResponse> {
  return requestJson<DashboardOverviewResponse>('api/v1/overview')
}

export function mcpServers(): Promise<McpServersResponse> {
  return requestJson<McpServersResponse>('api/v1/mcp/servers')
}

export function mcpDetect(url: string): Promise<McpDetectResponse> {
  const body: McpDetectRequest = { url }
  return requestJson<McpDetectResponse>('api/v1/mcp/detect', {
    method: 'POST',
    body: JSON.stringify(body),
  })
}

export function mcpAdd(request: McpAddRequest): Promise<McpMutationResponse> {
  return requestJson<McpMutationResponse>('api/v1/mcp/servers', {
    method: 'POST',
    body: JSON.stringify(request),
  })
}

export function mcpSetHeaders(serverName: string, headers: McpHeaderInput[]): Promise<McpMutationResponse> {
  const body: McpHeadersRequest = { headers }
  return requestJson<McpMutationResponse>(`api/v1/mcp/servers/${encodeURIComponent(serverName)}/headers`, {
    method: 'PATCH',
    body: JSON.stringify(body),
  })
}

export function mcpStartOAuth(serverName: string): Promise<McpOAuthStartResponse> {
  return requestJson<McpOAuthStartResponse>(`api/v1/mcp/servers/${encodeURIComponent(serverName)}/oauth/start`, {
    method: 'POST',
    body: JSON.stringify({}),
  })
}

export function mcpRemove(serverName: string): Promise<McpMutationResponse> {
  return requestJson<McpMutationResponse>(`api/v1/mcp/servers/${encodeURIComponent(serverName)}`, {
    method: 'DELETE',
  })
}

export function overview(): Promise<OverviewResponse> {
  return requestJson<OverviewResponse>('api/v1/activity/overview')
}

export function runDetail(runId: string): Promise<RunDetailResponse> {
  return requestJson<RunDetailResponse>(`api/v1/runs/${encodeURIComponent(runId)}`)
}

export function learningOverview(): Promise<LearningOverviewResponse> {
  return requestJson<LearningOverviewResponse>('api/v1/knowledge/learning/overview')
}

export function usageOverview(): Promise<UsageOverviewResponse> {
  return requestJson<UsageOverviewResponse>('api/v1/usage')
}

export function skillsOverview(): Promise<SkillsResponse> {
  return requestJson<SkillsResponse>('api/v1/knowledge/skills')
}

export function skillDetail(skillName: string): Promise<SkillDetailResponse> {
  return requestJson<SkillDetailResponse>(`api/v1/knowledge/skills/${encodeURIComponent(skillName)}`)
}

export function setSkillPinned(skillName: string, pinned: boolean): Promise<PinSkillResponse> {
  const body: PinSkillRequest = { pinned }
  return requestJson<PinSkillResponse>(`api/v1/knowledge/skills/${encodeURIComponent(skillName)}/pin`, {
    method: 'PATCH',
    body: JSON.stringify(body),
  })
}

export function identityFiles(): Promise<IdentityResponse> {
  return requestJson<IdentityResponse>('api/v1/identity')
}

export function identityFile(fileName: string): Promise<IdentityFileResponse> {
  return requestJson<IdentityFileResponse>(`api/v1/identity/${encodeURIComponent(fileName)}`)
}

export function doctorStatus(): Promise<DoctorResponse> {
  return requestJson<DoctorResponse>('api/v1/health/doctor')
}

export function sandboxStats(): Promise<SandboxStatsResponse> {
  return requestJson<SandboxStatsResponse>('api/v1/health/sandbox')
}

async function requestJson<T>(path: string, options: RequestInit = {}): Promise<T> {
  const headers = new Headers(options.headers)
  headers.set('Authorization', `tma ${window.Telegram?.WebApp?.initData ?? ''}`)
  if (!headers.has('Accept')) {
    headers.set('Accept', 'application/json')
  }
  if (options.body !== undefined && options.body !== null && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json')
  }

  let response: Response
  try {
    response = await fetch(path, {
      ...options,
      headers,
    })
  } catch {
    throw new DashboardApiError('Network unavailable', 0)
  }

  if (!response.ok) {
    throw await apiErrorFromResponse(response)
  }

  return (await response.json()) as T
}

async function apiErrorFromResponse(response: Response): Promise<DashboardApiError> {
  const fallback = response.status === 401 || response.status === 403
    ? 'Dashboard locked'
    : `Request failed (${response.status})`

  const body = await parseErrorBody(response)
  const detail = body?.detail?.trim()
  const code = body?.error?.trim()
  return new DashboardApiError(detail || code || fallback, response.status, code || undefined)
}

async function parseErrorBody(response: Response): Promise<ApiErrorBody | null> {
  const contentType = response.headers.get('content-type') ?? ''
  if (!contentType.includes('application/json')) {
    return null
  }

  try {
    return (await response.json()) as ApiErrorBody
  } catch {
    return null
  }
}
