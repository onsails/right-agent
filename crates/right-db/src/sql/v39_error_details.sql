CREATE TABLE IF NOT EXISTS error_details (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  chat_id    INTEGER NOT NULL,
  thread_id  INTEGER NOT NULL,
  raw_json   TEXT    NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_error_details_created_at
  ON error_details (created_at);
