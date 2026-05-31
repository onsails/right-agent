CREATE TABLE IF NOT EXISTS skill_spend (
  id             INTEGER PRIMARY KEY AUTOINCREMENT,
  skill_name     TEXT NOT NULL,
  kind           TEXT NOT NULL CHECK (kind IN ('create','patch','maintain','usage')),
  cost_usd       REAL NOT NULL DEFAULT 0.0,
  cache_read     INTEGER NOT NULL DEFAULT 0,
  cache_creation INTEGER NOT NULL DEFAULT 0,
  invocation_id  TEXT,
  ts             TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_skill_spend_skill_kind ON skill_spend(skill_name, kind);
CREATE INDEX IF NOT EXISTS idx_skill_spend_ts ON skill_spend(ts);

CREATE TABLE IF NOT EXISTS learning_skip (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  reason        TEXT NOT NULL,
  intended_kind TEXT,
  chat_id       INTEGER,
  thread_id     INTEGER,
  ts            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_learning_skip_reason_ts ON learning_skip(reason, ts);
