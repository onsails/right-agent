export interface ApiErrorBody {
  error: string
  detail: string | null
}

export interface BootstrapResponse {
  agent: string
  api_version: string
  refresh_interval_secs: number
  user_id: number
  features: DashboardFeatures
}

export interface DashboardFeatures {
  readonly: boolean
  commands_enabled: boolean
}

export interface OverviewResponse {
  agent: string
  generated_at: string
  refresh_interval_secs: number
  summary: OverviewSummary
  crons: CronCard[]
  active: ActiveActivity
}

export interface OverviewSummary {
  cron_count: number
  active_cron_count: number
  failed_recent_cron_count: number
  today_cost_usd: number
}

export interface CronCard {
  job_name: string
  schedule: string
  recurring: boolean
  run_at: string | null
  target_chat_id: number | null
  target_thread_id: number | null
  max_budget_usd: number
  last_run: RunSummary | null
  recent_runs: RunSummary[]
}

export interface ActiveActivity {
  foreground: ForegroundActivity[]
  background: RunSummary[]
}

export interface ForegroundActivity {
  chat_id: number
  thread_id: number
  turn_id: number
}

export interface RunSummary {
  id: string
  kind: string
  producer_ref: string | null
  status: string
  started_at: string | null
  finished_at: string | null
  exit_code: number | null
  delivery_status: string
  cost_usd: number | null
}

export interface RunDetailResponse {
  run: RunSummary
  summary: string | null
  notify_json: unknown | null
  no_notify_reason: string | null
  log: {
    available: boolean
    path: string | null
    lines: string[]
    truncated: boolean
  }
}
