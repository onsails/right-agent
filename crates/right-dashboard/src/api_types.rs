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
    pub learning_metrics: bool,
    pub learning_evidence_snippets: bool,
    pub learning_commands: bool,
    pub activity: bool,
    pub knowledge_learning: bool,
    pub knowledge_skills: bool,
    pub usage: bool,
    pub identity: bool,
    pub doctor: bool,
    pub sandbox_stats: bool,
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
pub struct DashboardOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub active_runs: i64,
    pub recent_failures: i64,
    pub today_cost_usd: f64,
    pub learning_candidates_24h: i64,
    pub doctor: OverviewDoctorStatus,
    pub sandbox: OverviewSandboxStatus,
    pub signals: Vec<DashboardSignal>,
    pub cost_learning_river: CostLearningRiver,
    pub warnings: Vec<DashboardDataWarning>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OverviewDoctorStatus {
    pub state: String,
    pub pass_count: i64,
    pub warn_count: i64,
    pub fail_count: i64,
    pub generated_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OverviewSandboxStatus {
    pub state: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub windows: Vec<UsageWindow>,
    pub selected_window: String,
    pub daily_series: Vec<UsageDailyPoint>,
    pub source_series: Vec<UsageSourceSeries>,
    pub warnings: Vec<DashboardDataWarning>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageWindow {
    pub key: String,
    pub label: String,
    pub sources: Vec<UsageSourceSummary>,
    pub total_cost_usd: f64,
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub per_model: Vec<UsageModelSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageSourceSummary {
    pub source: String,
    pub cost_usd: f64,
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub per_model: Vec<UsageModelSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageModelSummary {
    pub model: String,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DashboardDataWarning {
    pub source: String,
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DashboardSignal {
    pub id: String,
    pub kind: String,
    pub severity: String,
    pub occurred_at: String,
    pub title: String,
    pub detail: Option<String>,
    pub source: Option<String>,
    pub cost_usd: Option<f64>,
    pub related_run_id: Option<String>,
    pub related_skill_name: Option<String>,
    pub related_report_id: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CostLearningRiver {
    pub window: String,
    pub points: Vec<CostLearningPoint>,
    pub series: Vec<CostLearningSeries>,
    pub markers: Vec<LearningMarker>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CostLearningPoint {
    pub bucket: String,
    pub total_cost_usd: f64,
    pub sources: Vec<UsageSourcePoint>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CostLearningSeries {
    pub source: String,
    pub points: Vec<CostSeriesPoint>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CostSeriesPoint {
    pub bucket: String,
    pub cost_usd: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningMarker {
    pub id: String,
    pub occurred_at: String,
    pub kind: String,
    pub label: String,
    pub severity: String,
    pub skill_name: Option<String>,
    pub source: Option<String>,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageDailyPoint {
    pub date: String,
    pub total_cost_usd: f64,
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
    pub web_fetch_requests: u64,
    pub sources: Vec<UsageSourcePoint>,
    pub models: Vec<UsageModelSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageSourcePoint {
    pub source: String,
    pub cost_usd: f64,
    pub subscription_cost_usd: f64,
    pub api_cost_usd: f64,
    pub turns: u64,
    pub invocations: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UsageSourceSeries {
    pub source: String,
    pub points: Vec<CostSeriesPoint>,
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
    pub delivery_required: bool,
    pub delivery_status: String,
    pub delivery_kind: Option<String>,
    pub run_note: Option<String>,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunDetailResponse {
    pub run: RunSummary,
    pub run_note: Option<String>,
    pub delivery: Option<serde_json::Value>,
    pub delivery_error: Option<String>,
    pub error_message: Option<String>,
    pub log: LogExcerpt,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LogExcerpt {
    pub available: bool,
    pub path: Option<String>,
    pub lines: Vec<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningCapabilities {
    pub learning_metrics: bool,
    pub learning_evidence_snippets: bool,
    pub learning_commands: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningOverviewResponse {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
    pub capabilities: LearningCapabilities,
    pub lifecycle: LearningLifecycle,
    pub flow_nodes: Vec<LearningFlowNode>,
    pub flow_edges: Vec<LearningFlowEdge>,
    pub recent_learning_signals: Vec<LearningSignalPoint>,
    pub warnings: Vec<DashboardDataWarning>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningFlowNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub count: i64,
    pub severity: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningFlowEdge {
    pub source: String,
    pub target: String,
    pub count: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningSignalPoint {
    pub id: String,
    pub occurred_at: String,
    pub kind: String,
    pub label: String,
    pub severity: String,
    pub skill_name: Option<String>,
    pub count: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningLifecycle {
    pub created_7d: i64,
    pub updated_7d: i64,
    pub failed_or_aborted_7d: i64,
    pub recent_successful_events: Vec<LearningEventSummary>,
    pub candidate_skill_names_7d: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningEventSummary {
    pub skill_name: String,
    pub action: String,
    pub status: String,
    pub message: Option<String>,
    pub summary: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillsResponse {
    pub agent: String,
    pub source: String,
    pub warning: Option<String>,
    pub groups: SkillGroups,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillGroups {
    pub core: Vec<SkillSummary>,
    pub learned: Vec<SkillSummary>,
    pub other: Vec<SkillSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub group: String,
    pub path: String,
    pub description: Option<String>,
    #[serde(default)]
    pub state: Option<SkillLifecycleState>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub created_by: Option<SkillCreatedBy>,
    #[serde(default)]
    pub use_count: i64,
    #[serde(default)]
    pub patch_count: i64,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub last_patched_at: Option<String>,
}

impl SkillSummary {
    pub fn new(name: String, group: String, path: String, description: Option<String>) -> Self {
        Self {
            name,
            group,
            path,
            description,
            state: None,
            pinned: false,
            created_by: None,
            use_count: 0,
            patch_count: 0,
            created_at: None,
            last_used_at: None,
            last_patched_at: None,
        }
    }

    pub fn apply_lifecycle(&mut self, row: &right_lifecycle::SkillLifecycleRow) {
        self.state = Some(row.state.into());
        self.pinned = row.pinned;
        self.created_by = Some(row.created_by.into());
        self.use_count = row.use_count;
        self.patch_count = row.patch_count;
        self.created_at = row
            .created_at
            .as_ref()
            .map(|timestamp| timestamp.to_rfc3339());
        self.last_used_at = row
            .last_used_at
            .as_ref()
            .map(|timestamp| timestamp.to_rfc3339());
        self.last_patched_at = row
            .last_patched_at
            .as_ref()
            .map(|timestamp| timestamp.to_rfc3339());
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLifecycleState {
    Active,
    Stale,
    Archived,
}

impl From<right_lifecycle::LifecycleState> for SkillLifecycleState {
    fn from(value: right_lifecycle::LifecycleState) -> Self {
        match value {
            right_lifecycle::LifecycleState::Active => Self::Active,
            right_lifecycle::LifecycleState::Stale => Self::Stale,
            right_lifecycle::LifecycleState::Archived => Self::Archived,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillCreatedBy {
    Foreground,
    ProbeWriter,
    Curator,
    Bundled,
}

impl From<right_lifecycle::CreatedBy> for SkillCreatedBy {
    fn from(value: right_lifecycle::CreatedBy) -> Self {
        match value {
            right_lifecycle::CreatedBy::Foreground => Self::Foreground,
            right_lifecycle::CreatedBy::ProbeWriter => Self::ProbeWriter,
            right_lifecycle::CreatedBy::Curator => Self::Curator,
            right_lifecycle::CreatedBy::Bundled => Self::Bundled,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillDetailResponse {
    pub agent: String,
    pub skill: SkillSummary,
    pub content_preview: String,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PinSkillRequest {
    pub pinned: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PinSkillResponse {
    pub skill_name: String,
    pub pinned: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentityResponse {
    pub agent: String,
    pub source: String,
    pub warning: Option<String>,
    pub files: Vec<IdentityFileSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentityFileResponse {
    pub agent: String,
    pub warning: Option<String>,
    pub file: IdentityFileSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IdentityFileSummary {
    pub name: String,
    pub source: String,
    pub path: String,
    pub exists: bool,
    pub content_preview: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DoctorResponse {
    pub agent: String,
    pub generated_at: String,
    pub pass_count: i64,
    pub warn_count: i64,
    pub fail_count: i64,
    pub pass: Vec<DoctorCheckResponse>,
    pub warn: Vec<DoctorCheckResponse>,
    pub fail: Vec<DoctorCheckResponse>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DoctorCheckResponse {
    pub name: String,
    pub status: String,
    pub detail: String,
    pub fix: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SandboxStatsResponse {
    pub agent: String,
    pub source: String,
    pub warning: Option<String>,
    pub disk: Option<SandboxDiskStats>,
    pub memory: Option<SandboxMemoryStats>,
    pub processes: Vec<SandboxProcess>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SandboxDiskStats {
    pub mount: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SandboxMemoryStats {
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub load_average_1m: Option<f64>,
    pub load_average_5m: Option<f64>,
    pub load_average_15m: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SandboxProcess {
    pub pid: i32,
    pub ppid: i32,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub rss_bytes: u64,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkillLifecycleOverviewResponse {
    pub agent: String,
    pub total_active: i64,
    pub total_stale: i64,
    pub total_archived: i64,
    pub pinned_count: i64,
    pub agent_created_active: i64,
    pub probe_writer_active: i64,
    pub curator_active: i64,
    pub foreground_active: i64,
    pub bundled_active: i64,
    pub recently_used: Vec<RecentSkill>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecentSkill {
    pub package_name: String,
    pub use_count: u64,
    pub last_used_at: Option<String>,
}

#[cfg(test)]
mod dashboard_v2_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dashboard_v2_bootstrap_features_serialize() {
        let features = DashboardFeatures {
            readonly: true,
            commands_enabled: false,
            learning_metrics: true,
            learning_evidence_snippets: false,
            learning_commands: false,
            activity: true,
            knowledge_learning: true,
            knowledge_skills: true,
            usage: true,
            identity: true,
            doctor: true,
            sandbox_stats: true,
        };

        let value = serde_json::to_value(&features).unwrap();
        assert_eq!(
            value,
            json!({
                "readonly": true,
                "commands_enabled": false,
                "learning_metrics": true,
                "learning_evidence_snippets": false,
                "learning_commands": false,
                "activity": true,
                "knowledge_learning": true,
                "knowledge_skills": true,
                "usage": true,
                "identity": true,
                "doctor": true,
                "sandbox_stats": true,
            })
        );
    }

    #[test]
    fn skill_lifecycle_summary_deserializes_minimal_legacy_json() {
        let summary: SkillSummary = serde_json::from_value(json!({
            "name": "rightx-oauth-debugging",
            "group": "learned",
            "path": ".claude/skills/rightx-oauth-debugging/SKILL.md",
            "description": "Learned OAuth flow."
        }))
        .unwrap();

        assert_eq!(summary.state, None);
        assert!(!summary.pinned);
        assert_eq!(summary.created_by, None);
        assert_eq!(summary.use_count, 0);
        assert_eq!(summary.patch_count, 0);
        assert_eq!(summary.created_at, None);
        assert_eq!(summary.last_used_at, None);
        assert_eq!(summary.last_patched_at, None);
    }

    #[test]
    fn dashboard_overview_serializes_expected_shape() {
        let response = DashboardOverviewResponse {
            agent: "alpha".to_owned(),
            generated_at: "2026-05-20T12:00:00Z".to_owned(),
            active_runs: 2,
            recent_failures: 1,
            today_cost_usd: 0.42,
            learning_candidates_24h: 3,
            doctor: OverviewDoctorStatus {
                state: "not_loaded".to_owned(),
                pass_count: 0,
                warn_count: 0,
                fail_count: 0,
                generated_at: None,
            },
            sandbox: OverviewSandboxStatus {
                state: "unknown".to_owned(),
                detail: None,
            },
            signals: vec![],
            cost_learning_river: CostLearningRiver {
                window: "last_30_days".to_owned(),
                points: vec![],
                series: vec![],
                markers: vec![],
            },
            warnings: vec![],
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(
            value,
            json!({
                "agent": "alpha",
                "generated_at": "2026-05-20T12:00:00Z",
                "active_runs": 2,
                "recent_failures": 1,
                "today_cost_usd": 0.42,
                "learning_candidates_24h": 3,
                "doctor": {
                    "state": "not_loaded",
                    "pass_count": 0,
                    "warn_count": 0,
                    "fail_count": 0,
                    "generated_at": null,
                },
                "sandbox": {
                    "state": "unknown",
                    "detail": null,
                },
                "signals": [],
                "cost_learning_river": {
                    "window": "last_30_days",
                    "points": [],
                    "series": [],
                    "markers": [],
                },
                "warnings": [],
            })
        );
    }

    #[test]
    fn dashboard_visual_overview_serializes_expected_shape() {
        let response = DashboardOverviewResponse {
            agent: "alpha".to_owned(),
            generated_at: "2026-05-23T10:00:00Z".to_owned(),
            active_runs: 1,
            recent_failures: 1,
            today_cost_usd: 1.25,
            learning_candidates_24h: 2,
            doctor: OverviewDoctorStatus {
                state: "not_loaded".to_owned(),
                pass_count: 0,
                warn_count: 0,
                fail_count: 0,
                generated_at: None,
            },
            sandbox: OverviewSandboxStatus {
                state: "configured".to_owned(),
                detail: Some("sandbox alpha".to_owned()),
            },
            signals: vec![DashboardSignal {
                id: "learning:rightx-debug:2026-05-23T09:00:00Z".to_owned(),
                kind: "learning_outcome".to_owned(),
                severity: "info".to_owned(),
                occurred_at: "2026-05-23T09:00:00Z".to_owned(),
                title: "Skill created".to_owned(),
                detail: Some("rightx-debug".to_owned()),
                source: Some("learning_probe_writer".to_owned()),
                cost_usd: None,
                related_run_id: None,
                related_skill_name: Some("rightx-debug".to_owned()),
                related_report_id: None,
            }],
            cost_learning_river: CostLearningRiver {
                window: "last_30_days".to_owned(),
                points: vec![CostLearningPoint {
                    bucket: "2026-05-23".to_owned(),
                    total_cost_usd: 1.25,
                    sources: vec![UsageSourcePoint {
                        source: "interactive".to_owned(),
                        cost_usd: 1.25,
                        subscription_cost_usd: 1.25,
                        api_cost_usd: 0.0,
                        turns: 1,
                        invocations: 1,
                    }],
                }],
                series: vec![CostLearningSeries {
                    source: "interactive".to_owned(),
                    points: vec![CostSeriesPoint {
                        bucket: "2026-05-23".to_owned(),
                        cost_usd: 1.25,
                    }],
                }],
                markers: vec![LearningMarker {
                    id: "marker:rightx-debug".to_owned(),
                    occurred_at: "2026-05-23T09:00:00Z".to_owned(),
                    kind: "skill_created".to_owned(),
                    label: "rightx-debug".to_owned(),
                    severity: "info".to_owned(),
                    skill_name: Some("rightx-debug".to_owned()),
                    source: Some("learning_probe_writer".to_owned()),
                    cost_usd: None,
                }],
            },
            warnings: vec![DashboardDataWarning {
                source: "curator_state".to_owned(),
                kind: "unavailable".to_owned(),
                message: "curator state row is absent".to_owned(),
            }],
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["signals"][0]["kind"], "learning_outcome");
        assert_eq!(
            value["cost_learning_river"]["points"][0]["bucket"],
            "2026-05-23"
        );
        assert_eq!(value["warnings"][0]["kind"], "unavailable");
    }

    #[test]
    fn usage_visual_series_serializes_expected_shape() {
        let response = UsageOverviewResponse {
            agent: "alpha".to_owned(),
            generated_at: "2026-05-23T10:00:00Z".to_owned(),
            windows: vec![],
            selected_window: "last_30_days".to_owned(),
            daily_series: vec![UsageDailyPoint {
                date: "2026-05-23".to_owned(),
                total_cost_usd: 1.25,
                subscription_cost_usd: 1.00,
                api_cost_usd: 0.25,
                turns: 2,
                invocations: 2,
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_tokens: 5,
                cache_read_tokens: 40,
                web_search_requests: 1,
                web_fetch_requests: 2,
                sources: vec![UsageSourcePoint {
                    source: "interactive".to_owned(),
                    cost_usd: 1.25,
                    subscription_cost_usd: 1.00,
                    api_cost_usd: 0.25,
                    turns: 2,
                    invocations: 2,
                }],
                models: vec![UsageModelSummary {
                    model: "sonnet".to_owned(),
                    cost_usd: 1.25,
                    input_tokens: 10,
                    output_tokens: 20,
                    cache_creation_tokens: 5,
                    cache_read_tokens: 40,
                }],
            }],
            source_series: vec![UsageSourceSeries {
                source: "interactive".to_owned(),
                points: vec![CostSeriesPoint {
                    bucket: "2026-05-23".to_owned(),
                    cost_usd: 1.25,
                }],
            }],
            warnings: vec![],
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["selected_window"], "last_30_days");
        assert_eq!(
            value["daily_series"][0]["sources"][0]["source"],
            "interactive"
        );
    }
}

#[cfg(test)]
mod learning_tests {
    use super::*;

    #[test]
    fn learning_overview_serializes_expected_shape() {
        let response = LearningOverviewResponse {
            agent: "right".to_owned(),
            generated_at: "2026-05-20T12:00:00Z".to_owned(),
            refresh_interval_secs: 5,
            capabilities: LearningCapabilities {
                learning_metrics: true,
                learning_evidence_snippets: false,
                learning_commands: false,
            },
            lifecycle: LearningLifecycle {
                created_7d: 1,
                updated_7d: 0,
                failed_or_aborted_7d: 0,
                recent_successful_events: vec![LearningEventSummary {
                    skill_name: "rightx-oauth-debugging".to_owned(),
                    action: "create".to_owned(),
                    status: "created".to_owned(),
                    message: Some("Learned OAuth callback verification.".to_owned()),
                    summary: Some("Reusable OAuth setup workflow.".to_owned()),
                    created_at: "2026-05-20T10:00:00Z".to_owned(),
                }],
                candidate_skill_names_7d: vec!["rightx-oauth-debugging".to_owned()],
            },
            flow_nodes: vec![LearningFlowNode {
                id: "skill_created".to_owned(),
                label: "Skills created".to_owned(),
                kind: "skill".to_owned(),
                count: 1,
                severity: "info".to_owned(),
            }],
            flow_edges: vec![LearningFlowEdge {
                source: "writer_applied_as_hinted".to_owned(),
                target: "skill_created".to_owned(),
                count: 1,
            }],
            recent_learning_signals: vec![LearningSignalPoint {
                id: "learning:1".to_owned(),
                occurred_at: "2026-05-20T10:00:00Z".to_owned(),
                kind: "skill_created".to_owned(),
                label: "rightx-oauth-debugging".to_owned(),
                severity: "info".to_owned(),
                skill_name: Some("rightx-oauth-debugging".to_owned()),
                count: 1,
            }],
            warnings: vec![],
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["capabilities"]["learning_metrics"], true);
        assert_eq!(value["capabilities"]["learning_evidence_snippets"], false);
        assert_eq!(value["capabilities"]["learning_commands"], false);
        assert!(value.get("funnel").is_none());
        assert!(value.get("quality").is_none());
        assert!(value.get("recent_reports").is_none());
        assert_eq!(value["lifecycle"]["created_7d"], 1);
        assert_eq!(value["flow_nodes"][0]["id"], "skill_created");
        assert_eq!(value["flow_edges"][0]["count"], 1);
        assert_eq!(value["recent_learning_signals"][0]["kind"], "skill_created");
    }
}
