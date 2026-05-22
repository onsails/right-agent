CREATE TABLE IF NOT EXISTS curator_state (
    agent_singleton_id        INTEGER PRIMARY KEY CHECK (agent_singleton_id = 1),
    last_run_at               TEXT,
    last_run_status           TEXT,
    consecutive_failures      INTEGER NOT NULL DEFAULT 0,
    circuit_open_until        TEXT,
    last_spike_evidence_json  TEXT
);
