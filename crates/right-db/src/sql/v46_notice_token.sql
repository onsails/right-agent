CREATE TABLE IF NOT EXISTS notice_token (
    -- Single-row table: the per-agent notice token. `CHECK (id = 1)` makes the
    -- single-row invariant structural so a race or stray insert cannot create a
    -- second, ambiguous token row.
    id INTEGER PRIMARY KEY CHECK (id = 1),
    token TEXT NOT NULL
);
