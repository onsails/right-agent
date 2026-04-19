CREATE TABLE IF NOT EXISTS pending_retains (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    content         TEXT NOT NULL,
    context         TEXT,
    document_id     TEXT,
    update_mode     TEXT,
    tags_json       TEXT,
    created_at      TEXT NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TEXT,
    last_error      TEXT,
    source          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pending_retains_created
    ON pending_retains(created_at);

CREATE TABLE IF NOT EXISTS memory_alerts (
    alert_type    TEXT PRIMARY KEY,
    first_sent_at TEXT NOT NULL
);
