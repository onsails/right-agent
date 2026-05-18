CREATE TABLE IF NOT EXISTS async_runs (
    id                  TEXT PRIMARY KEY,
    kind                TEXT NOT NULL,
    producer_ref         TEXT,
    source_session_id    TEXT,
    run_session_id       TEXT NOT NULL,
    target_chat_id       INTEGER NOT NULL,
    target_thread_id     INTEGER,
    status              TEXT NOT NULL,
    handoff_state        TEXT,
    started_at           TEXT,
    finished_at          TEXT,
    exit_code            INTEGER,
    log_path             TEXT,
    summary              TEXT,
    notify_json          TEXT,
    no_notify_reason     TEXT,
    error_json           TEXT,
    delivery_required    INTEGER NOT NULL,
    delivery_status      TEXT NOT NULL,
    delivery_attempts    INTEGER NOT NULL DEFAULT 0,
    delivered_at         TEXT,
    last_delivery_error  TEXT,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_async_runs_kind_producer_started
    ON async_runs(kind, producer_ref, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_async_runs_delivery
    ON async_runs(delivery_required, delivery_status, status, finished_at);

CREATE INDEX IF NOT EXISTS idx_async_runs_target_status
    ON async_runs(target_chat_id, target_thread_id, status);

CREATE INDEX IF NOT EXISTS idx_async_runs_run_session
    ON async_runs(run_session_id);
