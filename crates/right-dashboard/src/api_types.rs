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
    pub funnel: LearningFunnel,
    pub quality: LearningQuality,
    pub health: LearningHealth,
    pub lifecycle: LearningLifecycle,
    pub recent_reports: Vec<LearningReportSummary>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningFunnel {
    pub signals_accepted_24h: i64,
    pub episodes_pending_24h: i64,
    pub episodes_selecting_24h: i64,
    pub episodes_selected_24h: i64,
    pub episodes_reviewing_24h: i64,
    pub episodes_reviewed_24h: i64,
    pub episodes_no_episode_24h: i64,
    pub episodes_insufficient_context_24h: i64,
    pub episodes_failed_24h: i64,
    pub reports_total_24h: i64,
    pub create_candidates_24h: i64,
    pub update_candidates_24h: i64,
    pub nothing_to_learn_24h: i64,
    pub failed_reviews_24h: i64,
    pub foreground_created_or_updated_7d: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningQuality {
    pub candidate_rate: Option<f64>,
    pub nothing_to_learn_rate: Option<f64>,
    pub create_count_24h: i64,
    pub update_count_24h: i64,
    pub high_confidence_count_24h: i64,
    pub medium_confidence_count_24h: i64,
    pub low_confidence_count_24h: i64,
    pub failed_count_24h: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningHealth {
    pub review_running: bool,
    pub daily_review_count: i64,
    pub daily_limit: i64,
    pub creation_review_interval: i64,
    pub tool_iters_since_review: i64,
    pub turns_since_review: i64,
    pub skill_issue_hints_since_review: i64,
    pub last_review_status: Option<String>,
    pub last_review_at: Option<String>,
    pub possibly_stuck: bool,
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
pub struct LearningReportSummary {
    pub id: i64,
    pub status: String,
    pub confidence: String,
    pub trigger_kind: String,
    pub candidate_skill_name: Option<String>,
    pub candidate_summary: Option<String>,
    pub telegram_notified: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningEpisodesResponse {
    pub agent: String,
    pub generated_at: String,
    pub episodes: Vec<LearningEpisodeSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningEpisodeSummary {
    pub id: i64,
    pub kind: String,
    pub seed_trigger_kind: String,
    pub seed_ref: String,
    pub status: String,
    pub target_chat_id: Option<i64>,
    pub target_thread_id: Option<i64>,
    pub start_ref: Option<String>,
    pub end_ref: Option<String>,
    pub confidence: Option<String>,
    pub context_incomplete: bool,
    pub last_evidence_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub reports: Vec<LearningReportSummary>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningEpisodeDetailResponse {
    pub episode: LearningEpisodeSummary,
    pub selector: Option<LearningSelectorDetail>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LearningReportDetailResponse {
    pub report: LearningReportSummary,
    pub episode: Option<LearningEpisodeDetail>,
    pub selector: Option<LearningSelectorDetail>,
    pub evidence: Vec<LearningEvidenceSnippet>,
    pub reviewer: LearningReviewerDetail,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningEpisodeDetail {
    pub id: i64,
    pub kind: String,
    pub seed_trigger_kind: String,
    pub status: String,
    pub start_ref: Option<String>,
    pub end_ref: Option<String>,
    pub boundary_rationale: Option<String>,
    pub confidence: Option<String>,
    pub context_incomplete: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningSelectorDetail {
    pub model: Option<String>,
    pub boundary_rationale: Option<String>,
    pub selected_message_refs: Vec<String>,
    pub selected_execution_event_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningEvidenceSnippet {
    pub ref_id: String,
    pub source: String,
    pub available: bool,
    pub trust_label: Option<String>,
    pub role: Option<String>,
    pub event_kind: Option<String>,
    pub tool_name: Option<String>,
    pub created_at: Option<String>,
    pub text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LearningReviewerDetail {
    pub status: String,
    pub confidence: String,
    pub candidate_skill_name: Option<String>,
    pub candidate_summary: Option<String>,
    pub evidence_refs: Vec<String>,
    pub user_notice_present: bool,
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
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillDetailResponse {
    pub agent: String,
    pub skill: SkillSummary,
    pub content_preview: String,
    pub truncated: bool,
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
            learning_evidence_snippets: true,
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
                "learning_evidence_snippets": true,
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
            })
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
                learning_evidence_snippets: true,
                learning_commands: false,
            },
            funnel: LearningFunnel {
                signals_accepted_24h: 2,
                episodes_pending_24h: 1,
                episodes_selecting_24h: 0,
                episodes_selected_24h: 0,
                episodes_reviewing_24h: 0,
                episodes_reviewed_24h: 1,
                episodes_no_episode_24h: 0,
                episodes_insufficient_context_24h: 0,
                episodes_failed_24h: 0,
                reports_total_24h: 1,
                create_candidates_24h: 1,
                update_candidates_24h: 0,
                nothing_to_learn_24h: 0,
                failed_reviews_24h: 0,
                foreground_created_or_updated_7d: 1,
            },
            quality: LearningQuality {
                candidate_rate: Some(1.0),
                nothing_to_learn_rate: Some(0.0),
                create_count_24h: 1,
                update_count_24h: 0,
                high_confidence_count_24h: 1,
                medium_confidence_count_24h: 0,
                low_confidence_count_24h: 0,
                failed_count_24h: 0,
            },
            health: LearningHealth {
                review_running: false,
                daily_review_count: 1,
                daily_limit: 12,
                creation_review_interval: 15,
                tool_iters_since_review: 3,
                turns_since_review: 1,
                skill_issue_hints_since_review: 0,
                last_review_status: Some("create_candidate".to_owned()),
                last_review_at: Some("2026-05-20T11:00:00Z".to_owned()),
                possibly_stuck: false,
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
            recent_reports: vec![LearningReportSummary {
                id: 7,
                status: "create_candidate".to_owned(),
                confidence: "high".to_owned(),
                trigger_kind: "learning_signal".to_owned(),
                candidate_skill_name: Some("rightx-oauth-debugging".to_owned()),
                candidate_summary: Some("Verify OAuth callback setup.".to_owned()),
                telegram_notified: true,
                created_at: "2026-05-20T11:00:00Z".to_owned(),
            }],
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["capabilities"]["learning_metrics"], true);
        assert_eq!(value["capabilities"]["learning_commands"], false);
        assert_eq!(value["funnel"]["create_candidates_24h"], 1);
        assert_eq!(value["funnel"]["episodes_insufficient_context_24h"], 0);
        assert_eq!(value["quality"]["candidate_rate"], 1.0);
        assert_eq!(
            value["recent_reports"][0]["candidate_skill_name"],
            "rightx-oauth-debugging"
        );
    }

    #[test]
    fn learning_report_detail_serializes_missing_snippet() {
        let response = LearningReportDetailResponse {
            report: LearningReportSummary {
                id: 9,
                status: "nothing_to_learn".to_owned(),
                confidence: "medium".to_owned(),
                trigger_kind: "effort_threshold".to_owned(),
                candidate_skill_name: None,
                candidate_summary: None,
                telegram_notified: false,
                created_at: "2026-05-20T11:00:00Z".to_owned(),
            },
            episode: Some(LearningEpisodeDetail {
                id: 4,
                kind: "foreground_thread".to_owned(),
                seed_trigger_kind: "effort_threshold".to_owned(),
                status: "reviewed".to_owned(),
                start_ref: Some("msg:1".to_owned()),
                end_ref: Some("exec:2".to_owned()),
                boundary_rationale: Some("Selected compact setup workflow.".to_owned()),
                confidence: Some("medium".to_owned()),
                context_incomplete: false,
            }),
            selector: Some(LearningSelectorDetail {
                model: Some("claude-sonnet-4-6".to_owned()),
                boundary_rationale: Some("Selected compact setup workflow.".to_owned()),
                selected_message_refs: vec!["msg:1".to_owned()],
                selected_execution_event_refs: vec!["exec:2".to_owned()],
            }),
            evidence: vec![LearningEvidenceSnippet {
                ref_id: "msg:404".to_owned(),
                source: "message".to_owned(),
                available: false,
                trust_label: None,
                role: None,
                event_kind: None,
                tool_name: None,
                created_at: None,
                text: None,
            }],
            reviewer: LearningReviewerDetail {
                status: "nothing_to_learn".to_owned(),
                confidence: "medium".to_owned(),
                candidate_skill_name: None,
                candidate_summary: None,
                evidence_refs: vec!["msg:404".to_owned()],
                user_notice_present: false,
            },
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["evidence"][0]["available"], false);
        assert!(value["evidence"][0]["text"].is_null());
        assert_eq!(value["reviewer"]["user_notice_present"], false);
    }
}
