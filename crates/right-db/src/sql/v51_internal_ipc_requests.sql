-- Durable deduplication for retryable internal.sock mutations.
--
-- One per-agent database, so `request_id` is globally unique within the
-- authenticated agent scope. `operation` prevents accidental reuse of one
-- id across two mutation kinds; `response_json` makes lost-response retries
-- return the exact original result without repeating writes.
CREATE TABLE IF NOT EXISTS internal_ipc_requests (
    request_id    TEXT PRIMARY KEY,
    operation     TEXT NOT NULL,
    response_json TEXT NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_internal_ipc_requests_created
ON internal_ipc_requests(created_at);
