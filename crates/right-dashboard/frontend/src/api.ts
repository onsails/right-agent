import type {
  ApiErrorBody,
  BootstrapResponse,
  LearningEpisodeDetailResponse,
  LearningEpisodesResponse,
  LearningOverviewResponse,
  LearningReportDetailResponse,
  OverviewResponse,
  RunDetailResponse,
  UsageOverviewResponse,
} from './types'

declare global {
  interface Window {
    Telegram?: {
      WebApp?: {
        initData?: string
        ready?: () => void
        expand?: () => void
      }
    }
  }
}

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

export function overview(): Promise<OverviewResponse> {
  return requestJson<OverviewResponse>('api/v1/overview')
}

export function runDetail(runId: string): Promise<RunDetailResponse> {
  return requestJson<RunDetailResponse>(`api/v1/runs/${encodeURIComponent(runId)}`)
}

export function learningOverview(): Promise<LearningOverviewResponse> {
  return requestJson<LearningOverviewResponse>('api/v1/knowledge/learning/overview')
}

export function learningEpisodes(): Promise<LearningEpisodesResponse> {
  return requestJson<LearningEpisodesResponse>('api/v1/knowledge/learning/episodes')
}

export function learningEpisodeDetail(episodeId: number): Promise<LearningEpisodeDetailResponse> {
  return requestJson<LearningEpisodeDetailResponse>(`api/v1/knowledge/learning/episodes/${encodeURIComponent(String(episodeId))}`)
}

export function learningReportDetail(reportId: number): Promise<LearningReportDetailResponse> {
  return requestJson<LearningReportDetailResponse>(`api/v1/knowledge/learning/reports/${encodeURIComponent(String(reportId))}`)
}

export function usageOverview(): Promise<UsageOverviewResponse> {
  return requestJson<UsageOverviewResponse>('api/v1/usage')
}

async function requestJson<T>(path: string): Promise<T> {
  let response: Response
  try {
    response = await fetch(path, {
      headers: {
        Authorization: `tma ${window.Telegram?.WebApp?.initData ?? ''}`,
        Accept: 'application/json',
      },
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
