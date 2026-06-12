CREATE TABLE IF NOT EXISTS thread_focus (
  chat_id        INTEGER NOT NULL,
  thread_id      INTEGER NOT NULL DEFAULT 0,
  operator_focus TEXT,
  agent_focus    TEXT,
  updated_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
  PRIMARY KEY (chat_id, thread_id)
);
