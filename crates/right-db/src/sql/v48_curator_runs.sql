CREATE TABLE IF NOT EXISTS curator_runs (
    id                     INTEGER PRIMARY KEY AUTOINCREMENT,
    run_at                 TEXT NOT NULL,
    trigger                TEXT NOT NULL,          -- cost_spike | skill_change | time_fallback
    trigger_evidence_json  TEXT,
    mode                   TEXT NOT NULL,          -- apply | report_only
    status                 TEXT NOT NULL,          -- success | failed | proposed
    cost_usd               REAL NOT NULL DEFAULT 0,
    cache_read             INTEGER NOT NULL DEFAULT 0,
    cache_creation         INTEGER NOT NULL DEFAULT 0,
    consolidations         INTEGER NOT NULL DEFAULT 0,  -- skills merged (absorbed_into set); subset of archives
    archives               INTEGER NOT NULL DEFAULT 0,  -- total skills archived this pass
    summary                TEXT,
    actions_json           TEXT NOT NULL DEFAULT '[]',
    invocation_id          TEXT,
    created_at             TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_curator_runs_run_at ON curator_runs(run_at);
