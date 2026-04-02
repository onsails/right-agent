# Phase 22: DB Schema - Context

**Gathered:** 2026-03-31
**Status:** Ready for planning

<domain>
## Phase Boundary

Add `telegram_sessions` table to `memory.db` via a V2 rusqlite_migration. No CRUD code — schema + migration only. Phase 25 implements CRUD against this table.

</domain>

<decisions>
## Implementation Decisions

### Schema: telegram_sessions table

- **D-01:** `chat_id INT NOT NULL` — not nullable. A session without a chat_id can never route a reply. Omission of NOT NULL in SES-01 was an oversight.
- **D-02:** `thread_id INT NOT NULL DEFAULT 0` — as per SES-01. Guards against thread_id=1 normalization bug.
- **D-03:** `root_session_id TEXT NOT NULL` — stores first-call session UUID; never updated on resume (SES-01, guards against CC bug #8069).
- **D-04:** `created_at TEXT NOT NULL DEFAULT (datetime('now'))` — same pattern as V1 schema.
- **D-05:** `last_used_at TEXT` — **nullable**. NULL on creation; updated to `datetime('now')` each time the session is resumed. NULL means "created, never resumed." Distinguishes fresh sessions from active ones.
- **D-06:** `UNIQUE(chat_id, thread_id)` — composite key preventing duplicate session rows per conversation thread.

### Multi-channel strategy

- **D-07:** `telegram_sessions` stays Telegram-specific. Other channels (Slack, Discord, webhooks) will have their own tables with channel-specific schemas. Each channel's semantics differ (e.g., Slack has no `thread_id`; webhooks may lack chat IDs entirely). No shared generic `sessions` table.

### Migration module placement

- **D-08:** Extend `memory/migrations.rs` — add `v2_telegram_sessions.sql` to `memory/sql/` and append `M::up(V2_SCHEMA)` to the migration vec. No new module for Phase 22. Phase 23-25 will introduce a `telegram/` module when CRUD is needed.

</decisions>

<specifics>
## Specific Ideas

- Future channels (Slack, Discord, webhooks) each get their own session table — not unified. Avoids forcing incompatible semantics into one schema.
- `last_used_at` nullable is semantically precise: NULL means the session was created but the user hasn't resumed it yet.

</specifics>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Schema requirement
- `.planning/REQUIREMENTS.md` §SES-01 — exact column spec for telegram_sessions V2 migration

### Existing migration infrastructure
- `crates/rightclaw/src/memory/migrations.rs` — current migration registration (V1); V2 appends here
- `crates/rightclaw/src/memory/sql/v1_schema.sql` — V1 schema conventions (column types, TEXT dates, `IF NOT EXISTS` guards)
- `crates/rightclaw/src/memory/mod.rs` — `open_db` / `open_connection` that apply migrations on DB open

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `migrations::MIGRATIONS` LazyLock: add `M::up(V2_SCHEMA)` as second element in the vec — migration order is positional
- `include_str!("sql/v2_telegram_sessions.sql")` pattern matches V1 exactly

### Established Patterns
- Column types: `INT` for integers, `TEXT` for strings and timestamps, `INTEGER PRIMARY KEY AUTOINCREMENT` for PK
- Timestamps: `TEXT NOT NULL DEFAULT (datetime('now'))` for always-set fields; plain `TEXT` for nullable/updated-later fields
- `CREATE TABLE IF NOT EXISTS` guard on all tables
- WAL mode + busy_timeout set in `open_db` — not needed in migration SQL itself

### Integration Points
- `open_db()` and `open_connection()` in `memory/mod.rs` call `MIGRATIONS.to_latest()` — V2 migration runs automatically on next DB open after code deploy
- Tests in `memory/mod.rs` verify `user_version` == N after migration; add test asserting `user_version == 2` and `telegram_sessions` table exists

</code_context>

<deferred>
## Deferred Ideas

- Multi-channel generic session table — rejected in favor of per-channel tables; each future channel creates its own migration
- Slack/Discord/webhook session tables — future phases when those channels are implemented

</deferred>

---

*Phase: 22-db-schema*
*Context gathered: 2026-03-31*
