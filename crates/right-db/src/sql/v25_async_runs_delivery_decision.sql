ALTER TABLE async_runs RENAME TO async_runs_old;

CREATE TABLE async_runs (
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
    run_note            TEXT,
    delivery_json       TEXT,
    error_json           TEXT,
    delivery_required    INTEGER NOT NULL,
    delivery_status      TEXT NOT NULL,
    delivery_attempts    INTEGER NOT NULL DEFAULT 0,
    delivered_at         TEXT,
    last_delivery_error  TEXT,
    created_at           TEXT NOT NULL,
    updated_at           TEXT NOT NULL
);

INSERT INTO async_runs (
    id, kind, producer_ref, source_session_id, run_session_id,
    target_chat_id, target_thread_id, status, handoff_state,
    started_at, finished_at, exit_code, log_path, run_note,
    delivery_json, error_json, delivery_required, delivery_status,
    delivery_attempts, delivered_at, last_delivery_error, created_at, updated_at
)
SELECT
    id, kind, producer_ref, source_session_id, run_session_id,
    target_chat_id, target_thread_id, status, handoff_state,
    started_at, finished_at, exit_code, log_path, summary,
    NULL, error_json, 0,
    CASE
      WHEN delivery_status IN ('delivered', 'superseded', 'failed') THEN delivery_status
      ELSE 'none'
    END,
    delivery_attempts, delivered_at, last_delivery_error, created_at, updated_at
FROM async_runs_old;

DROP TABLE async_runs_old;

CREATE INDEX IF NOT EXISTS idx_async_runs_kind_producer_started
    ON async_runs(kind, producer_ref, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_async_runs_delivery
    ON async_runs(delivery_required, delivery_status, status, finished_at);

CREATE INDEX IF NOT EXISTS idx_async_runs_target_status
    ON async_runs(target_chat_id, target_thread_id, status);

CREATE INDEX IF NOT EXISTS idx_async_runs_run_session
    ON async_runs(run_session_id);
