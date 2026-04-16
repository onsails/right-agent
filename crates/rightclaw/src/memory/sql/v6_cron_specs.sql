-- V6: Cron spec storage in DB (replaces crons/*.yaml files).
CREATE TABLE IF NOT EXISTS cron_specs (
    job_name       TEXT PRIMARY KEY,
    schedule       TEXT NOT NULL,
    prompt         TEXT NOT NULL,
    lock_ttl       TEXT,
    max_budget_usd REAL NOT NULL DEFAULT 1.0,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
