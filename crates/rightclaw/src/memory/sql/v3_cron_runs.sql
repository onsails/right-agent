-- V3 schema: cron_runs history table
-- Decision D-04 (Phase 27)
CREATE TABLE IF NOT EXISTS cron_runs (
    id          TEXT    PRIMARY KEY,          -- UUID
    job_name    TEXT    NOT NULL,
    started_at  TEXT    NOT NULL,             -- ISO8601 UTC
    finished_at TEXT,                         -- NULL while running
    exit_code   INTEGER,                      -- NULL while running
    status      TEXT    NOT NULL,             -- 'running' | 'success' | 'failed'
    log_path    TEXT    NOT NULL              -- absolute path to log file
);
