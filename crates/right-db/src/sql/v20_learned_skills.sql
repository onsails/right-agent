CREATE TABLE IF NOT EXISTS skill_learning_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  invocation_id TEXT NOT NULL,
  agent_name TEXT NOT NULL,
  action TEXT NOT NULL CHECK (action IN ('create', 'update')),
  skill_name TEXT NOT NULL,
  phase TEXT NOT NULL CHECK (phase IN ('start', 'finish')),
  status TEXT CHECK (status IS NULL OR status IN ('created', 'updated', 'aborted', 'failed')),
  reason TEXT,
  message TEXT,
  summary TEXT,
  event_refs_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_skill_learning_events_invocation
  ON skill_learning_events(invocation_id);

CREATE INDEX IF NOT EXISTS idx_skill_learning_events_skill
  ON skill_learning_events(skill_name);

CREATE TABLE IF NOT EXISTS skill_nudge_signals (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  invocation_id TEXT NOT NULL,
  agent_name TEXT NOT NULL,
  root_session_id TEXT,
  chat_id INTEGER,
  thread_id INTEGER,
  signal_kind TEXT NOT NULL CHECK (signal_kind IN ('learning', 'skill_issue')),
  payload_json TEXT NOT NULL,
  accepted_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_skill_nudge_signals_invocation
  ON skill_nudge_signals(invocation_id);

CREATE TABLE IF NOT EXISTS skill_nudge_state (
  agent_name TEXT PRIMARY KEY,
  tool_iters_since_review INTEGER NOT NULL DEFAULT 0,
  turns_since_review INTEGER NOT NULL DEFAULT 0,
  skill_issue_hints_since_review INTEGER NOT NULL DEFAULT 0,
  last_review_at TEXT,
  review_running INTEGER NOT NULL DEFAULT 0
);
