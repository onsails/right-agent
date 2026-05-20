use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BootstrapResponse {
    pub agent: String,
    pub api_version: String,
    pub refresh_interval_secs: u64,
    pub user_id: i64,
    pub features: DashboardFeatures,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DashboardFeatures {
    pub readonly: bool,
    pub commands_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
    pub summary: OverviewSummary,
    pub crons: Vec<CronCard>,
    pub active: ActiveActivity,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OverviewSummary {
    pub cron_count: usize,
    pub active_cron_count: usize,
    pub failed_recent_cron_count: usize,
    pub today_cost_usd: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CronCard {
    pub job_name: String,
    pub schedule: String,
    pub recurring: bool,
    pub run_at: Option<String>,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub max_budget_usd: f64,
    pub last_run: Option<RunSummary>,
    pub recent_runs: Vec<RunSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActiveActivity {
    pub foreground: Vec<ForegroundActivity>,
    pub background: Vec<RunSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ForegroundActivity {
    pub chat_id: i64,
    pub thread_id: i64,
    pub turn_id: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub kind: String,
    pub producer_ref: Option<String>,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub exit_code: Option<i64>,
    pub delivery_status: String,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunDetailResponse {
    pub run: RunSummary,
    pub summary: Option<String>,
    pub notify_json: Option<serde_json::Value>,
    pub no_notify_reason: Option<String>,
    pub log: LogExcerpt,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LogExcerpt {
    pub available: bool,
    pub path: Option<String>,
    pub lines: Vec<String>,
    pub truncated: bool,
}
