CREATE TABLE IF NOT EXISTS execution_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_name TEXT NOT NULL,
  root_session_id TEXT,
  invocation_id TEXT,
  turn_id INTEGER,
  async_run_id TEXT,
  cron_job_name TEXT,
  cron_run_id TEXT,
  seq INTEGER NOT NULL,
  event_kind TEXT NOT NULL CHECK (event_kind IN ('assistant_text','thinking','tool_call','tool_result','tool_error','invocation_result','other')),
  tool_name TEXT,
  content_json TEXT NOT NULL DEFAULT '{}',
  content_text TEXT NOT NULL DEFAULT '',
  trust_label TEXT NOT NULL CHECK (trust_label IN ('primary','secondary','low_trust')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_execution_events_agent_session_seq ON execution_events(agent_name, root_session_id, seq);
CREATE INDEX IF NOT EXISTS idx_execution_events_invocation ON execution_events(invocation_id);
CREATE INDEX IF NOT EXISTS idx_execution_events_async_run ON execution_events(async_run_id);
CREATE INDEX IF NOT EXISTS idx_execution_events_cron_run ON execution_events(cron_run_id);

CREATE TABLE IF NOT EXISTS learning_episodes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('foreground_thread','async_continuation','cron_run')),
  seed_trigger_kind TEXT NOT NULL CHECK (seed_trigger_kind IN ('learning_signal','skill_issue_signal','effort_threshold','cron','async_result')),
  seed_ref TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending','selecting','selected','reviewing','reviewed','no_episode','insufficient_context','failed')),
  target_chat_id INTEGER,
  target_thread_id INTEGER,
  start_ref TEXT,
  end_ref TEXT,
  message_refs_json TEXT NOT NULL DEFAULT '[]',
  execution_event_refs_json TEXT NOT NULL DEFAULT '[]',
  selector_model TEXT,
  selector_output_json TEXT,
  boundary_rationale TEXT,
  confidence TEXT CHECK (confidence IN ('low','medium','high')),
  context_incomplete INTEGER NOT NULL DEFAULT 0 CHECK (context_incomplete IN (0, 1)),
  episode_hash TEXT,
  ready_after TEXT NOT NULL,
  last_evidence_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_learning_episodes_hash ON learning_episodes(agent_name, episode_hash) WHERE episode_hash IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_learning_episodes_seed ON learning_episodes(agent_name, kind, seed_trigger_kind, seed_ref);
CREATE INDEX IF NOT EXISTS idx_learning_episodes_ready ON learning_episodes(agent_name, status, ready_after);
