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

export interface OverviewResponse {
  agent: string
  generated_at: string
  refresh_interval_secs: number
  summary: OverviewSummary
  crons: CronCard[]
  active: ActiveActivity
}

export interface DashboardOverviewResponse {
  agent: string
  generated_at: string
  active_runs: number
  recent_failures: number
  today_cost_usd: number
  learning_candidates_24h: number
  doctor: OverviewDoctorStatus
  sandbox: OverviewSandboxStatus
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

export interface UsageOverviewResponse {
  agent: string
  generated_at: string
  windows: UsageWindow[]
}

export interface UsageWindow {
  key: string
  label: string
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

export interface UsageModelSummary {
  model: string
  cost_usd: number
  input_tokens: number
  output_tokens: number
  cache_creation_tokens: number
  cache_read_tokens: number
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

export interface LearningCapabilities {
  learning_metrics: boolean
  learning_evidence_snippets: boolean
  learning_commands: boolean
}

export interface LearningOverviewResponse {
  agent: string
  generated_at: string
  refresh_interval_secs: number
  capabilities: LearningCapabilities
  funnel: LearningFunnel
  quality: LearningQuality
  health: LearningHealth
  lifecycle: LearningLifecycle
  recent_reports: LearningReportSummary[]
}

export interface LearningFunnel {
  signals_accepted_24h: number
  episodes_pending_24h: number
  episodes_selecting_24h: number
  episodes_selected_24h: number
  episodes_reviewing_24h: number
  episodes_reviewed_24h: number
  episodes_no_episode_24h: number
  episodes_insufficient_context_24h: number
  episodes_failed_24h: number
  reports_total_24h: number
  create_candidates_24h: number
  update_candidates_24h: number
  nothing_to_learn_24h: number
  failed_reviews_24h: number
  foreground_created_or_updated_7d: number
}

export interface LearningQuality {
  candidate_rate: number | null
  nothing_to_learn_rate: number | null
  create_count_24h: number
  update_count_24h: number
  high_confidence_count_24h: number
  medium_confidence_count_24h: number
  low_confidence_count_24h: number
  failed_count_24h: number
}

export interface LearningHealth {
  review_running: boolean
  daily_review_count: number
  daily_limit: number
  creation_review_interval: number
  tool_iters_since_review: number
  turns_since_review: number
  skill_issue_hints_since_review: number
  last_review_status: string | null
  last_review_at: string | null
  possibly_stuck: boolean
}

export interface LearningLifecycle {
  created_7d: number
  updated_7d: number
  failed_or_aborted_7d: number
  recent_successful_events: LearningEventSummary[]
  candidate_skill_names_7d: string[]
}

export interface LearningEventSummary {
  skill_name: string
  action: string
  status: string
  message: string | null
  summary: string | null
  created_at: string
}

export interface LearningReportSummary {
  id: number
  status: string
  confidence: string
  trigger_kind: string
  candidate_skill_name: string | null
  candidate_summary: string | null
  telegram_notified: boolean
  created_at: string
}

export interface LearningEpisodesResponse {
  agent: string
  generated_at: string
  episodes: LearningEpisodeSummary[]
}

export interface LearningEpisodeSummary {
  id: number
  kind: string
  seed_trigger_kind: string
  seed_ref: string
  status: string
  target_chat_id: number | null
  target_thread_id: number | null
  start_ref: string | null
  end_ref: string | null
  confidence: string | null
  context_incomplete: boolean
  last_evidence_at: string
  created_at: string
  updated_at: string
  reports: LearningReportSummary[]
}

export interface LearningEpisodeDetailResponse {
  episode: LearningEpisodeSummary
  selector: LearningSelectorDetail | null
}

export interface LearningReportDetailResponse {
  report: LearningReportSummary
  episode: LearningEpisodeDetail | null
  selector: LearningSelectorDetail | null
  evidence: LearningEvidenceSnippet[]
  reviewer: LearningReviewerDetail
}

export interface LearningEpisodeDetail {
  id: number
  kind: string
  seed_trigger_kind: string
  status: string
  start_ref: string | null
  end_ref: string | null
  boundary_rationale: string | null
  confidence: string | null
  context_incomplete: boolean
}

export interface LearningSelectorDetail {
  model: string | null
  boundary_rationale: string | null
  selected_message_refs: string[]
  selected_execution_event_refs: string[]
}

export interface LearningEvidenceSnippet {
  ref_id: string
  source: string
  available: boolean
  trust_label: string | null
  role: string | null
  event_kind: string | null
  tool_name: string | null
  created_at: string | null
  text: string | null
}

export interface LearningReviewerDetail {
  status: string
  confidence: string
  candidate_skill_name: string | null
  candidate_summary: string | null
  evidence_refs: string[]
  user_notice_present: boolean
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

export interface SkillSummary {
  name: string
  group: string
  path: string
  description: string | null
}

export interface SkillDetailResponse {
  agent: string
  skill: SkillSummary
  content_preview: string
  truncated: boolean
}
