CREATE TABLE IF NOT EXISTS auth_tokens (
    token TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
