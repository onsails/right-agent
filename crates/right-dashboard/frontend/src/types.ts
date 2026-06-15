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
  learning_metrics: boolean
  learning_evidence_snippets: boolean
  learning_commands: boolean
  activity: boolean
  knowledge_learning: boolean
  knowledge_skills: boolean
  usage: boolean
  identity: boolean
  doctor: boolean
  sandbox_stats: boolean
}

export type McpAuthMode = 'oauth' | 'headers' | 'url_as_is'

export interface McpServerSummary {
  name: string
  url: string | null
  status: string
  tool_count: number
  auth_type: string | null
  header_names: string[]
  protected: boolean
  last_connect_error?: string | null
  last_attempt_at?: string | null
  last_success_at?: string | null
}

export interface McpServersResponse {
  servers: McpServerSummary[]
}

export interface McpDetectRequest {
  url: string
}

export interface McpDetectResponse {
  bare_url: string
  oauth_discovered: boolean
  recommended_mode: McpAuthMode
  reason: string
  oauth: {
    resource: string
    scopes: string[]
    authorization_endpoint: string
    token_endpoint: string
    registration_endpoint: string | null
  } | null
}

export interface McpHeaderInput {
  name: string
  value: string
}

export interface McpAddRequest {
  name: string
  url: string
  mode: McpAuthMode
  headers: McpHeaderInput[]
}

export interface McpHeadersRequest {
  headers: McpHeaderInput[]
}

export interface McpMutationResponse {
  ok: boolean
}

export interface McpOAuthStartResponse {
  auth_url: string
  flow_id: string
}

export type McpOAuthFlowStatus = 'pending' | 'succeeded' | 'failed' | 'expired' | 'unknown'

export interface McpOAuthStatusResponse {
  flow_id: string
  server_name: string | null
  status: McpOAuthFlowStatus
  message: string | null
  updated_at: string
}

export interface OverviewResponse {
  agent: string
  generated_at: string
  refresh_interval_secs: number
  summary: OverviewSummary
  crons: CronCard[]
  failed_runs: RunSummary[]
  active: ActiveActivity
}

export interface DashboardOverviewResponse {
  agent: string
  generated_at: string
  active_runs: number
  recent_failures: number
  recent_failed_runs: RunSummary[]
  today_cost_usd: number
  learning_candidates_24h: number
  doctor: OverviewDoctorStatus
  sandbox: OverviewSandboxStatus
  signals: DashboardSignal[]
  cost_learning_river: CostLearningRiver
  warnings: DashboardDataWarning[]
}

export interface OverviewDoctorStatus {
  state: string
  pass_count: number
  warn_count: number
  fail_count: number
  generated_at: string | null
}

export interface OverviewSandboxStatus {
  state: string
  detail: string | null
}

export type UsageRange = 'today' | 'last_3_days' | 'last_7_days' | 'last_30_days' | 'all_time'

export interface UsageOverviewResponse {
  agent: string
  generated_at: string
  timezone: string
  selected_range: UsageRange
  window: UsageWindow
  windows: UsageWindow[]
  selected_window: string
  daily_series: UsageDailyPoint[]
  source_series: UsageSourceSeries[]
  cron_jobs: UsageCronJobSummary[]
  warnings: DashboardDataWarning[]
}

export interface UsageWindow {
  key: string
  label: string
  range_start: string | null
  range_end: string
  range_label: string
  sources: UsageSourceSummary[]
  total_cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
  web_search_requests: number
  web_fetch_requests: number
  per_model: UsageModelSummary[]
  budget_skip_count: number
}

export interface UsageSourceSummary {
  source: string
  cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
  web_search_requests: number
  web_fetch_requests: number
  per_model: UsageModelSummary[]
}

export interface UsageCronJobSummary {
  job_name: string
  cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
  web_search_requests: number
  web_fetch_requests: number
  per_model: UsageModelSummary[]
}

export interface UsageModelSummary {
  model: string
  cost_usd: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}

export interface DashboardDataWarning {
  source: string
  kind: string
  message: string
}

export interface DashboardSignal {
  id: string
  kind: string
  severity: string
  occurred_at: string
  title: string
  detail: string | null
  source: string | null
  cost_usd: number | null
  related_run_id: string | null
  related_skill_name: string | null
  related_report_id: number | null
}

export interface CostLearningRiver {
  window: string
  points: CostLearningPoint[]
  series: CostLearningSeries[]
  markers: LearningMarker[]
}

export interface CostLearningPoint {
  bucket: string
  total_cost_usd: number
  sources: UsageSourcePoint[]
}

export interface CostLearningSeries {
  source: string
  points: CostSeriesPoint[]
}

export interface CostSeriesPoint {
  bucket: string
  cost_usd: number
}

export interface LearningMarker {
  id: string
  occurred_at: string
  kind: string
  label: string
  severity: string
  skill_name: string | null
  source: string | null
  cost_usd: number | null
}

export interface UsageDailyPoint {
  date: string
  total_cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
  web_search_requests: number
  web_fetch_requests: number
  sources: UsageSourcePoint[]
  models: UsageModelSummary[]
}

export interface UsageSourcePoint {
  source: string
  cost_usd: number
  subscription_cost_usd: number
  api_cost_usd: number
  turns: number
  invocations: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
}

export interface UsageSourceSeries {
  source: string
  points: CostSeriesPoint[]
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
  schedule_human: string
  recurring: boolean
  run_at: string | null
  next_run_at: string | null
  target_chat_id: number | null
  target_thread_id: number | null
  max_budget_usd: number
  spend_24h_usd: number
  spend_7d_usd: number
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
  delivery_required: boolean
  delivery_status: string
  delivery_kind: string | null
  run_note: string | null
  cost_usd: number | null
}

export interface RunDetailResponse {
  run: RunSummary
  run_note: string | null
  delivery: unknown | null
  delivery_error: string | null
  error_message: string | null
  log: {
    available: boolean
    path: string | null
    lines: string[]
    truncated: boolean
  }
}

export interface LearningCapabilities {
  learning_metrics: boolean
  learning_evidence_snippets: boolean
  learning_commands: boolean
}

export interface CuratorRunSummary {
  run_at: string
  trigger: string
  mode: string
  status: string
  cost_usd: number
  consolidations: number
  archives: number
  summary: string | null
}

export interface CuratorConsolidation {
  absorbed: string
  umbrella: string
}

export interface LearningOverviewResponse {
  agent: string
  generated_at: string
  refresh_interval_secs: number
  capabilities: LearningCapabilities
  lifecycle: LearningLifecycle
  flow_nodes: LearningFlowNode[]
  flow_edges: LearningFlowEdge[]
  recent_learning_signals: LearningSignalPoint[]
  warnings: DashboardDataWarning[]
  curator_runs: CuratorRunSummary[]
  curator_consolidations: CuratorConsolidation[]
}

export interface LearningFlowNode {
  id: string
  label: string
  kind: string
  count: number
  severity: string
}

export interface LearningFlowEdge {
  source: string
  target: string
  count: number
}

export interface LearningSignalPoint {
  id: string
  occurred_at: string
  kind: string
  label: string
  severity: string
  detail: string | null
  skill_name: string | null
  count: number
}

export interface LearningLifecycle {
  created_7d: number
  updated_7d: number
  failed_7d: number
  refused_7d: number
  recent_successful_events: LearningEventSummary[]
  recent_failed_events: LearningEventSummary[]
  recent_refused_events: LearningEventSummary[]
  candidate_skill_names_7d: string[]
}

export interface LearningEventSummary {
  id: number
  skill_name: string
  action: string
  status: string
  message: string | null
  summary: string | null
  created_at: string
}

export interface SkillsResponse {
  agent: string
  source: string
  warning: string | null
  groups: SkillGroups
}

export interface SkillGroups {
  core: SkillSummary[]
  learned: SkillSummary[]
  other: SkillSummary[]
}

export type SkillLifecycleState = 'active' | 'stale' | 'archived'

export type SkillCreatedBy = 'foreground' | 'probe_writer' | 'curator' | 'bundled'

export interface SkillSummary {
  name: string
  group: string
  path: string
  description: string | null
  state: SkillLifecycleState | null
  pinned: boolean
  created_by: SkillCreatedBy | null
  use_count: number
  patch_count: number
  created_at: string | null
  last_used_at: string | null
  last_patched_at: string | null
  learn_cost_usd: number
  fix_cost_usd: number
  usage_cost_usd: number
  cache_read_tokens: number
  cache_creation_tokens: number
}

export interface SkillDetailResponse {
  agent: string
  skill: SkillSummary
  content_preview: string
  truncated: boolean
}

export interface PinSkillRequest {
  pinned: boolean
}

export interface PinSkillResponse {
  skill_name: string
  pinned: boolean
}

export interface IdentityResponse {
  agent: string
  source: string
  warning: string | null
  files: IdentityFileSummary[]
}

export interface IdentityFileResponse {
  agent: string
  warning: string | null
  file: IdentityFileSummary
}

export interface IdentityFileSummary {
  name: string
  source: string
  path: string
  exists: boolean
  content_preview: string | null
  truncated: boolean
}

export interface DoctorResponse {
  agent: string
  generated_at: string
  pass_count: number
  warn_count: number
  fail_count: number
  pass: DoctorCheckResponse[]
  warn: DoctorCheckResponse[]
  fail: DoctorCheckResponse[]
}

export interface DoctorCheckResponse {
  name: string
  status: string
  detail: string
  fix: string | null
}

export interface SandboxStatsResponse {
  agent: string
  source: string
  warning: string | null
  disk: SandboxDiskStats | null
  memory: SandboxMemoryStats | null
  processes: SandboxProcess[]
}

export interface SandboxDiskStats {
  mount: string
  total_bytes: number
  used_bytes: number
  available_bytes: number
  used_percent: number
}

export interface SandboxMemoryStats {
  total_bytes: number | null
  available_bytes: number | null
  used_bytes: number | null
  load_average_1m: number | null
  load_average_5m: number | null
  load_average_15m: number | null
}

export interface SandboxProcess {
  pid: number
  ppid: number
  cpu_percent: number
  memory_percent: number
  rss_bytes: number
  command: string
}

export interface ProviderView {
  name: string
  type: string
  label: string | null
  env_var: string
  generic: ProviderGenericBody | null
  updated_at: string | null
  composed: boolean | null
  status:
    | { kind: 'healthy' }
    | { kind: 'missing' }
    | { kind: 'gateway_error'; message: string }
    | { kind: 'unknown_builtin'; slug: string }
}

export interface ProviderProfileView {
  type: string
  env_var: string
  display_name: string
  category: string
}

export interface ProviderGenericBody {
  env_var: string
  upstream_hosts: string[]
  upstream_path_prefix?: string
}

export interface ProviderCreateBody {
  type: string
  label?: string
  credential: string
  generic?: ProviderGenericBody
}
