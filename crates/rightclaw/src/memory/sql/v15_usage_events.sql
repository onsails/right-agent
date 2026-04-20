-- V15 schema: usage_events — per-invocation CC token/cost telemetry.
-- One row per `claude -p` invocation, written when the `result` stream event is observed.

CREATE TABLE IF NOT EXISTS usage_events (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                     TEXT    NOT NULL,       -- ISO8601 UTC
    source                 TEXT    NOT NULL,       -- 'interactive' | 'cron'
    chat_id                INTEGER,                -- NULL for cron
    thread_id              INTEGER,                -- 0 if no thread, NULL for cron
    job_name               TEXT,                   -- NULL for interactive
    session_uuid           TEXT    NOT NULL,
    total_cost_usd         REAL    NOT NULL,
    num_turns              INTEGER NOT NULL,
    input_tokens           INTEGER NOT NULL DEFAULT 0,
    output_tokens          INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens      INTEGER NOT NULL DEFAULT 0,
    web_search_requests    INTEGER NOT NULL DEFAULT 0,
    web_fetch_requests     INTEGER NOT NULL DEFAULT 0,
    model_usage_json       TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_usage_events_ts ON usage_events (ts);
CREATE INDEX IF NOT EXISTS idx_usage_events_source_ts ON usage_events (source, ts);
