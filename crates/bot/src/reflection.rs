//! Error reflection — on a failed CC invocation, run a short `--resume`-d pass
//! so the agent itself produces a user-friendly summary.
//!
//! Callers: `telegram::worker` (interactive) and `cron` (scheduled).
//! See: docs/superpowers/specs/2026-04-21-error-reflection-design.md

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use crate::telegram::stream::StreamEvent;

/// Classifies the failure we are reflecting on. Drives the human-readable
/// reason text inserted into the SYSTEM_NOTICE prompt.
#[derive(Debug, Clone)]
pub enum FailureKind {
    /// Process was killed by the 600-second safety net in worker.
    SafetyTimeout { limit_secs: u64 },
    /// CC reported `--max-budget-usd` exhaustion.
    BudgetExceeded { limit_usd: f64 },
    /// CC reported `--max-turns` exhaustion.
    MaxTurns { limit: u32 },
    /// Non-zero exit code with no auth-error classification.
    NonZeroExit { code: i32 },
}

/// Discriminator for where the reflection originated — decides how the usage
/// row is written and helps /usage render a breakdown.
#[derive(Debug, Clone)]
pub enum ParentSource {
    Worker { chat_id: i64, thread_id: i64 },
    Cron { job_name: String },
}

/// Resource caps for a single reflection invocation.
#[derive(Debug, Clone, Copy)]
pub struct ReflectionLimits {
    pub max_turns: u32,
    pub max_budget_usd: f64,
    pub process_timeout: Duration,
}

impl ReflectionLimits {
    pub const WORKER: Self = Self {
        max_turns: 3,
        max_budget_usd: 0.20,
        process_timeout: Duration::from_secs(90),
    };
    pub const CRON: Self = Self {
        max_turns: 5,
        max_budget_usd: 0.40,
        process_timeout: Duration::from_secs(180),
    };
}

/// All inputs required to run one reflection pass.
#[derive(Debug, Clone)]
pub struct ReflectionContext {
    pub session_uuid: String,
    pub failure: FailureKind,
    pub ring_buffer_tail: VecDeque<StreamEvent>,
    pub limits: ReflectionLimits,
    pub agent_name: String,
    pub agent_dir: PathBuf,
    pub ssh_config_path: Option<PathBuf>,
    pub resolved_sandbox: Option<String>,
    pub db_path: PathBuf,
    pub parent_source: ParentSource,
    pub model: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReflectionError {
    #[error("reflection spawn failed: {0}")]
    Spawn(String),
    #[error("reflection timed out after {0:?}")]
    Timeout(Duration),
    #[error("reflection CC exited with code {code}: {detail}")]
    NonZeroExit { code: i32, detail: String },
    #[error("reflection output parse failed: {0}")]
    Parse(String),
    #[error("reflection I/O failed: {0}")]
    Io(#[from] std::io::Error),
}
