CREATE TABLE IF NOT EXISTS skill_review_reports (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_name TEXT NOT NULL,
  source_invocation_id TEXT NOT NULL,
  root_session_id TEXT,
  chat_id INTEGER,
  thread_id INTEGER,
  trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('learning_signal', 'skill_issue_signal', 'effort_threshold')),
  status TEXT NOT NULL CHECK (status IN ('nothing_to_learn', 'create_candidate', 'update_candidate', 'failed')),
  confidence TEXT NOT NULL CHECK (confidence IN ('low', 'medium', 'high')),
  candidate_skill_name TEXT,
  candidate_summary TEXT,
  evidence_refs_json TEXT NOT NULL DEFAULT '[]',
  review_output_json TEXT NOT NULL,
  telegram_notified INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_skill_review_reports_agent_created
  ON skill_review_reports(agent_name, created_at);

CREATE INDEX IF NOT EXISTS idx_skill_review_reports_invocation
  ON skill_review_reports(source_invocation_id);
