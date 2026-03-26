# Pitfalls Research: v2.3 Memory System

**Domain:** SQLite-backed memory added to existing multi-agent Rust runtime (RightClaw) alongside flat-file MEMORY.md
**Researched:** 2026-03-26
**Confidence:** HIGH (active research area, codebase audited, NeurIPS 2025 published attacks, SQLite official docs verified)

---

## Critical Pitfalls

### Pitfall 1: Memory Entries Injected Into the Agent's Own System Prompt via MEMORY.md

**What goes wrong:**
RightClaw agents already have `MEMORY.md` auto-injected into the Claude Code system prompt at session start. If the memory skill writes entries that also get appended to `MEMORY.md` (or to any file CC auto-loads), every memory entry becomes part of future system prompts. A crafted memory entry — stored via any input the agent processes (a webpage, an email, a document, another agent's output) — can persist as a prompt injection payload that executes in every subsequent session.

This is not theoretical: MINJA (NeurIPS 2025) demonstrated 95%+ injection success rates against memory-backed LLM agents by poisoning the memory retrieval path. The "temporally decoupled" nature of the attack means the agent behaves normally for days, then executes the injected instruction when an unrelated query triggers the poisoned recall.

Specific RightClaw exposure:
- The memory skill writes to SQLite. Fine so far.
- If `rightclaw memory` CLI reads entries and formats them into `MEMORY.md` for CC injection — that's the injection surface.
- If the memory skill appends a "memory summary" to `MEMORY.md` after `forget`/`store` — any crafted content reaches the system prompt.
- If CC's own auto-memory (`~/.claude/projects/.../MEMORY.md`) and the agent's `MEMORY.md` are both injected, an attacker only needs to poison one layer.

**Why it happens:**
The convenience of "surface all memory at session start" conflates two separate concerns: what the agent knows (semantic memory) and what the agent should act on (active context). Writing to an auto-injected file collapses that boundary.

**How to avoid:**
- Keep SQLite memory completely separate from `MEMORY.md`. The memory skill stores to SQLite; `MEMORY.md` remains a human-authored file that `rightclaw up` never modifies.
- Memory recall must be on-demand via the `recall`/`search` skill commands — never auto-injected at session start without explicit user instruction.
- Sanitize all memory entries before storage: scan for prompt injection patterns (imperative instruction fragments, "ignore previous instructions", exfiltration URLs, invisible Unicode). Reject on match; store a redacted tombstone if audit trail is needed.
- Mark the provenance of every entry (agent-written vs. user-written vs. external-ingested). Apply stricter injection scanning to external-provenance entries.

**Warning signs:**
- The memory skill SKILL.md has a "summarize to MEMORY.md" step.
- `rightclaw up` generates or appends to `MEMORY.md` from the SQLite store.
- Memory entries contain imperative sentences starting with "From now on", "Always", "Never", or "Ignore".

**Phase to address:** Phase 1 (skill design). The boundary between SQLite-only memory and CC-injected files must be defined before writing a single line of skill code.

---

### Pitfall 2: No Schema Migration Strategy — DB Locked to Phase 1 Schema Forever

**What goes wrong:**
The initial schema is shipped with `CREATE TABLE IF NOT EXISTS`. No migration table, no version tracking. Phase 1 ships with columns `(id, content, tags, created_at)`. Phase 2 adds `provenance` column. Phase 3 adds `embedding BLOB` for semantic search.

Without a migration runner, the options at each phase are:
1. `ALTER TABLE ADD COLUMN` — works for additive changes, breaks for renames/drops/type changes
2. Drop and recreate — destroys all agent memory on upgrade
3. Conditional `PRAGMA table_info` checks everywhere — runtime complexity explosion

In production, real agents accumulate memory over months. A migration that silently nukes the DB (or fails and leaves the DB in an inconsistent state mid-migration) is a hard bug to recover from.

Real precedent: OpenCode beta migration from JSON to SQLite (Drizzle ORM) broke all 30+ plugins that read the old files. For RightClaw, the equivalent is a `rightclaw up` that fails because the agent's DB schema doesn't match the binary's expected schema.

**Why it happens:**
"We'll deal with migrations when we need them" — but the first schema is always wrong, and there is always a phase 2.

**How to avoid:**
- Use `refinery` (crate: `refinery`, v0.9) with `rusqlite` from day one. Embed SQL migration files via `refinery::embed_migrations!`. Each migration is `V{N}__{name}.sql` in a `migrations/` directory.
- Refinery creates a `refinery_schema_history` table and tracks applied migrations. On `rightclaw up`, run `runner.run(&mut conn)` before any other DB operations. Additive, idempotent.
- Never use `CREATE TABLE IF NOT EXISTS` without also running refinery — the two approaches conflict (refinery tracks which migrations ran; manual `IF NOT EXISTS` bypasses that).
- Write migration `V1__initial.sql` in Phase 1. Every subsequent schema change is a new migration file, never a change to an existing one.
- Add a `rightclaw doctor` check: open each agent DB, run `PRAGMA user_version`, compare against expected version, warn if out of date.

**Warning signs:**
- No `migrations/` directory in the project after Phase 1.
- Schema created directly in Rust with a hardcoded `CREATE TABLE` string.
- `PRAGMA user_version` returns 0 on all agent DBs (no version tracking in place).

**Phase to address:** Phase 1 (DB layer foundation). Refinery setup must be the first commit that touches SQLite, before any schema is defined.

---

### Pitfall 3: SQLite File Locking on Concurrent Agent Restarts

**What goes wrong:**
process-compose restarts agents automatically (or on `rightclaw restart`). When an agent is restarting, there is a window where:
1. Old Claude process is terminating (may hold an open `rusqlite::Connection`)
2. New Claude process is starting (calls `sqlite3_open()` on the same file)
3. The memory skill writes to the DB during session teardown hooks

If the old process did not call `sqlite3_close()` cleanly (e.g., killed by SIGKILL, or bubblewrap terminated the sandbox abruptly), the WAL file (`memory.db-wal`) and shared memory file (`memory.db-shm`) remain on disk. The new process opens in WAL mode, triggers WAL recovery, and holds `WAL_RECOVER_LOCK` — blocking all readers and writers until recovery completes.

Worse: if the memory skill is a Claude Code skill (not a native Rust binary), it opens and closes connections via tool calls. Each tool call may open a new connection. If two tool calls overlap (e.g., `store` and `recall` invoked near-simultaneously), both hold write intent on the same file. Without `busy_timeout` set, the second call fails instantly with `SQLITE_BUSY` — and the skill gets no memory written with no user-visible error.

**Why it happens:**
SQLite's default `busy_timeout` is 0 — fail immediately on contention. The documentation does not make this obvious. Developers assume WAL mode "handles concurrency" without realizing WAL only allows concurrent readers, not concurrent writers.

**How to avoid:**
- Enable WAL mode unconditionally: `PRAGMA journal_mode=WAL;` on first open.
- Set `busy_timeout` to 5000ms: `PRAGMA busy_timeout=5000;` on every connection open. This covers transient contention during restart windows.
- Open the DB with `Connection::open_with_flags()` using the default flags (no `FULL_MUTEX` — rusqlite handles thread safety statically).
- Keep all writes in short, explicit transactions (`BEGIN IMMEDIATE`... `COMMIT`). Never hold a write transaction across a tool call boundary.
- In the memory skill, treat `SQLITE_BUSY` after timeout as a hard error with a user-visible message: "Memory DB is locked — another operation is in progress. Retry in a moment."
- Add a `rightclaw doctor` check: attempt to open each agent DB and run `SELECT 1`. Report if locked.

**Warning signs:**
- `memory.db-wal` and `memory.db-shm` files persist after all agents are stopped (indicates unclean shutdown).
- `store` or `recall` silently returns empty on a freshly restarted agent.
- The memory skill exits with "database is locked" after a `rightclaw restart` sequence.

**Phase to address:** Phase 1 (DB layer) — WAL and busy_timeout are connection-initialization code, not afterthoughts. Phase 2 (CLI) — the `rightclaw memory` command also opens connections and must follow the same patterns.

---

### Pitfall 4: Memory Bloat — Unbounded Growth With No Eviction

**What goes wrong:**
Each agent stores memories indefinitely. After weeks of operation, a busy agent accumulates thousands of entries. The memory skill's `search` command returns top-K by recency or relevance, but the DB grows without bound:
1. Storage: not critical for local SQLite, but a 100MB DB per agent is surprising to users.
2. Performance: FTS5 indexing degrades at scale; full-text searches slow down.
3. Injection surface: a larger memory store has more attack surface for poisoned entries.
4. Skill UX: `recall` that returns 500 irrelevant entries is worse than useless.

The specific pattern seen in OpenClaw: "month one is clean, month three has temporary notes accumulating, month six is a 20,000-token monster nobody wants to touch." With SQLite, the monster doesn't appear in the file but it's still there, silently.

**Why it happens:**
"Forget nothing" feels safe. Eviction feels risky (what if we delete something important?). The cost of bloat is diffuse (slow queries, large file) vs. the cost of eviction being acute (deleted memory, user complaint).

**How to avoid:**
- Define retention policy at schema design time, not as a future feature. Add `expires_at TIMESTAMP NULL` to the initial schema. Entries with a non-null `expires_at` are automatically excluded from search after expiry.
- Add `importance` (0-100 INT) and `access_count` (INT) columns. Implement LRU-style eviction: when entry count exceeds a configurable threshold (default: 1000), prune the N lowest-importance, least-recently-accessed entries older than 30 days.
- Expose the threshold in `agent.yaml` under `memory.max_entries` and `memory.retention_days`. Document defaults.
- The `rightclaw memory` CLI's `list` command should display DB size and entry count. This makes bloat visible before it's a problem.
- Add a `VACUUM` step in `rightclaw up`: after connecting to each agent DB, run `PRAGMA auto_vacuum=INCREMENTAL; PRAGMA incremental_vacuum;` to reclaim space from deleted rows.

**Warning signs:**
- No `expires_at` or `importance` column in V1 schema.
- `rightclaw memory list` shows entry count but no size or age distribution.
- No `max_entries` config in `agent.yaml`.

**Phase to address:** Phase 1 (schema design) for the columns; Phase 2 (CLI) for the visibility tooling.

---

### Pitfall 5: MEMORY.md and SQLite Dual-Truth Conflict

**What goes wrong:**
The agent has two memory stores: the existing `MEMORY.md` (CC-native, human-authored, auto-injected into system prompt) and the new SQLite store (machine-managed, skill-accessible). If the memory skill can write to both, or if `rightclaw memory` can modify `MEMORY.md`, two problems emerge:

1. **Drift**: `MEMORY.md` says "user prefers dark mode"; SQLite says "user prefers light mode" (written later). Which is true? The agent has no reconciliation mechanism.

2. **Double injection**: CC injects `MEMORY.md` into the system prompt. The memory skill also surfaces relevant SQLite entries. The agent receives the same fact twice — once statically, once via recall. At best, this wastes context tokens. At worst, if the two versions diverge, the agent resolves the contradiction incorrectly.

3. **Undefined update path**: a user edits `MEMORY.md` manually. The memory skill has no knowledge of this change. If `rightclaw memory import MEMORY.md` exists, who is source of truth after import?

OpenClaw issue #26949 (MEMORY.md vs memory_search) documents this exact dual-injection problem as a live bug.

**Why it happens:**
The two systems serve different purposes (human-authored facts vs. agent-managed episodic memory) but share the conceptual space of "what the agent remembers." The boundaries are not enforced at a system level.

**How to avoid:**
- Define the contract explicitly and enforce it in code:
  - `MEMORY.md` = human-authored, static, injected into system prompt. The memory skill NEVER writes to it. `rightclaw up` NEVER modifies it.
  - SQLite = agent-managed, dynamic, accessible only via explicit skill calls (`store`/`recall`/`search`/`forget`). Never auto-injected.
- Add a note to the top of the generated `MEMORY.md` scaffold: "This file is human-authored. For agent-managed memory, use the `/rightmem` skill."
- The `rightclaw memory import` command (if added later) must be an explicit migration step with a warning, not an automatic sync.
- The memory skill's SKILL.md must document: "Entries stored here are NOT injected into the system prompt at session start. Use `recall` to surface them when needed."

**Warning signs:**
- The memory skill has a step that appends to `MEMORY.md` after `store`.
- `rightclaw up` generates or updates `MEMORY.md` from SQLite.
- The skill SKILL.md says "memories are automatically loaded at startup."

**Phase to address:** Phase 1 (skill design). The boundary must be documented in the SKILL.md and enforced by never writing to `MEMORY.md` from either the skill or the CLI.

---

### Pitfall 6: Audit Trail Without Immutability — Logs That Lie

**What goes wrong:**
The milestone requires "full audit trail: timestamps + provenance on every entry." A naive implementation:
```sql
CREATE TABLE memories (
    id INTEGER PRIMARY KEY,
    content TEXT,
    created_at TIMESTAMP,
    updated_at TIMESTAMP,
    deleted_at TIMESTAMP  -- soft delete
);
```

This schema allows UPDATE and DELETE on rows, meaning the audit trail can be silently modified. A memory entry can be overwritten (losing the original), and the `created_at` / `deleted_at` approach does not prevent an UPDATE to those timestamp fields.

More critically: if the memory skill supports `forget`, and `forget` does a hard DELETE, the audit trail has gaps. "What was stored at time T" is unanswerable.

**Why it happens:**
Mutable audit tables feel like normal database design. The immutability constraint is non-obvious unless you've built compliance systems before.

**How to avoid:**
- Never UPDATE or DELETE from the `memories` table. Implement soft-delete via a separate `memory_events` table:
  ```sql
  CREATE TABLE memory_events (
      id          INTEGER PRIMARY KEY,
      memory_id   INTEGER NOT NULL,
      event_type  TEXT NOT NULL,  -- 'store', 'forget', 'update'
      content     TEXT,           -- NULL for 'forget' events
      provenance  TEXT NOT NULL,  -- 'agent', 'user', 'cli'
      agent_name  TEXT NOT NULL,
      created_at  TIMESTAMP NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
  );
  ```
  "Current state" is derived by replaying events for a given `memory_id`. `forget` inserts a `'forget'` event — it never deletes rows.
- Add SQLite triggers to prevent UPDATE/DELETE on `memory_events`:
  ```sql
  CREATE TRIGGER prevent_event_update BEFORE UPDATE ON memory_events
  BEGIN SELECT RAISE(ABORT, 'memory_events is append-only'); END;
  CREATE TRIGGER prevent_event_delete BEFORE DELETE ON memory_events
  BEGIN SELECT RAISE(ABORT, 'memory_events is append-only'); END;
  ```
- Expose audit history via `rightclaw memory history <entry-id>`.

**Warning signs:**
- Schema has an `updated_at` column on the memories table (implies UPDATE semantics).
- `forget` is implemented as `DELETE FROM memories WHERE id = ?`.
- No `memory_events` table or equivalent event log.

**Phase to address:** Phase 1 (schema design). Append-only event sourcing must be baked into V1 — retrofitting it requires a data migration.

---

## Moderate Pitfalls

### Pitfall 7: Missing `busy_timeout` Causes Silent Memory Loss in the Skill

**What goes wrong:**
The memory skill is a Claude Code skill — it calls Bash commands or a Rust binary. If the binary opens a `rusqlite::Connection` without setting `busy_timeout`, any contention (even a short lock from another tool call) returns `SQLITE_BUSY` immediately. The skill gets a Rust error. Claude Code sees a non-zero exit code or an error message. The model may not surface this to the user (it may silently retry, interpret it as "nothing found", or continue without the memory write).

Silent memory loss is worse than a visible error because the agent believes it stored the memory, but it didn't.

**How to avoid:**
- Every connection open must immediately run `PRAGMA busy_timeout=5000;`.
- The memory binary must exit with code 1 and a clear stderr message on `SQLITE_BUSY` after timeout.
- The SKILL.md must handle the error case explicitly: if the memory binary exits non-zero, the skill must surface the error message to the user rather than continuing silently.

**Phase to address:** Phase 1 (DB layer). Treat this as part of the connection initialization checklist alongside WAL mode.

---

### Pitfall 8: CLI Command Opens DB While Agent Also Has It Open

**What goes wrong:**
`rightclaw memory list` (CLI) opens the agent's DB while the agent is running (agent has an open connection via the memory skill). In WAL mode, concurrent readers are fine. But `rightclaw memory delete <id>` acquires a write lock. If the agent is in the middle of a `store` or `recall` operation, the delete races the agent write. With `busy_timeout=5000ms` on both sides, one will retry and succeed — but if the agent's connection is long-lived (a persistent connection in the skill binary), the CLI will consistently time out.

**How to avoid:**
- The CLI and the skill must both use short transactions. Neither should hold an open write transaction for more than milliseconds.
- For `rightclaw memory delete`, acquire a write lock with `BEGIN IMMEDIATE`, make the change, `COMMIT`. If `SQLITE_BUSY` after timeout, print "Agent is currently writing memory. Try again in a moment." Do not retry in a loop — that's a busy wait.
- If the skill uses a persistent binary with a long-lived connection, restructure it to open, transact, close per operation.

**Phase to address:** Phase 2 (CLI commands). The CLI must be written with the assumption that agents are always running and the DB is always contended.

---

### Pitfall 9: Per-Agent DB Path Not Derived From Agent Dir — Cross-Agent Leakage

**What goes wrong:**
If the DB path is computed incorrectly (e.g., a bug where the path defaults to `~/.rightclaw/memory.db` instead of `~/.rightclaw/agents/<name>/memory.db`), multiple agents share one DB. All agents read and write each other's memories. In a multi-agent setup (the RightClaw core use case), this is a complete isolation failure.

The right-claw design has per-agent HOME isolation via `HOME=$AGENT_DIR`. If the memory skill opens its DB via `$HOME/memory.db` inside the skill, and `$HOME` is correctly set by the shell wrapper to the agent dir, the path is correct. But if the Rust CLI opens the DB via a hardcoded or config-derived path, it might not use the same derivation.

**How to avoid:**
- The DB path MUST be `<agent_dir>/memory.db` always. The agent dir is the single source of truth (same as agent's HOME).
- The CLI derives DB path via `agent_dir.join("memory.db")` — the same logic used to find `IDENTITY.md`, `agent.yaml`, etc.
- The skill uses `$HOME/memory.db` inside the agent sandbox (correct, because `$HOME` = agent dir).
- Add a test: create two agents, store a memory in agent-A, list memories for agent-B, assert empty.

**Phase to address:** Phase 1 (DB path derivation) and Phase 2 (CLI). Test isolation explicitly.

---

### Pitfall 10: Refinery Migration Checksums Break on Migration File Edits

**What goes wrong:**
Refinery checksums each migration file when it first runs. If a developer edits `V1__initial.sql` after it has already been applied to an agent DB, the next `rightclaw up` fails with "checksum mismatch for migration V1". This is correct behavior — but developers editing existing migration files during development is common.

More specifically: once a RightClaw version ships and users have agent DBs with applied migrations, any edit to a shipped migration file means all existing users get a checksum error on next launch. The agent fails to start because `runner.run()` returns an error before the DB is accessible.

**How to avoid:**
- Treat migration files as immutable once shipped (part of the release). New changes are always new migration files.
- During development (before first release), it is acceptable to reset: `rm ~/.rightclaw/agents/*/memory.db` and re-run. Document this as the dev workflow.
- After first release: never edit existing migration files. Use `V2__add_column.sql`, etc.
- The `rightclaw doctor` check for "DB schema out of date" should distinguish between "migration checksum mismatch" (developer edited a shipped file — actionable fix: reset DB) and "migration pending" (new version, forward-migrate — automatic fix via `rightclaw up`).

**Phase to address:** Phase 1 (development discipline). Formalize the no-edit-after-ship rule in project documentation.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| `CREATE TABLE IF NOT EXISTS` without refinery | Simpler Phase 1 | Cannot migrate schema without data loss | Never — refinery from day one costs 30 minutes |
| Write to MEMORY.md from memory skill | "Memories appear automatically at session start" | Prompt injection surface; dual-truth conflict with human-authored MEMORY.md | Never |
| Hard DELETE for `forget` | Simpler SQL | No audit trail; cannot reconstruct history | Never if audit trail is a requirement |
| Default `busy_timeout=0` (SQLite default) | Zero code | Silent memory loss on any contention | Never for a production skill |
| Single DB for all agents at `~/.rightclaw/memory.db` | Simpler path computation | Complete cross-agent isolation failure | Never |
| No eviction policy | No complex pruning logic | Unbounded DB growth; performance degradation at scale | Acceptable in Phase 1 IF eviction columns are in the schema (can add logic later) |
| Store raw external content without injection scanning | Simpler skill | Memory poisoning attack surface | Never; scan is a one-time implementation |
| Embed schema as a Rust string literal | No migration files to manage | Schema becomes opaque; migrations impossible | Never — use refinery SQL files |

---

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| CC native sandbox + SQLite file | Assuming `$HOME/memory.db` is accessible inside bwrap sandbox | Agent dir is both `$HOME` and inside `allowWrite` by default — DB at `$HOME/memory.db` is accessible. Verify `allowWrite` includes agent dir |
| rusqlite in a CC skill (Bash invocation) | Opening a connection, doing work, exiting without explicit close | Rust `Drop` closes the connection on scope exit — this is fine. But signal handling (SIGKILL) bypasses Drop. WAL recovery handles this on next open |
| process-compose + DB file | process-compose restart policy sends SIGKILL after timeout | SIGKILL leaves WAL files open. WAL recovery on next open is the designed recovery path — but only works if WAL mode was enabled before the kill |
| `rightclaw memory` CLI + running agent | CLI and agent share the DB file | Use WAL + `busy_timeout=5000` on both sides; keep CLI writes in short transactions |
| Refinery + rusqlite | Calling `runner.run(&mut conn)` on a `Connection` that already has WAL pragma set | Safe — refinery runs migrations inside transactions; WAL mode persists across connections |
| FTS5 full-text search | Building FTS5 index on a table that already has millions of rows | FTS5 must be created at schema definition time (V1 migration). Retrofitting FTS5 onto an existing large table requires a rebuild — expensive |

---

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Auto-injecting SQLite entries into system prompt | Stored memory entry becomes a prompt injection payload; persists across sessions | Never auto-inject. On-demand recall only. Sanitize entries on store. |
| No injection scanning before `store` | Adversarial content (from web, tools, other agents) plants durable instructions | Scan for imperative injection patterns before writing to DB |
| `forget` deletes rows (no audit trail) | Cannot detect or reconstruct a poisoned memory after it is "forgotten" | Append-only event log; `forget` inserts a forget event, never deletes |
| Shared DB across agents | Agent A reads Agent B's memories, leaking context across agent boundaries | Per-agent DB path derived from agent dir; isolation test in CI |
| DB file readable by all users | Another user on the machine reads the agent's memory store | DB file created with 0600 permissions; enforce via `OpenFlags` + `fs::set_permissions` after open |
| Storing raw external content without sanitization | Indirect prompt injection via ingested external data (Palo Alto Unit 42 attack pattern) | Sanitize all externally-sourced content before writing; mark provenance; trust scoring |

---

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| `store` returns success but DB was locked (silent write failure) | Agent believes it stored memory; recall returns nothing | Memory binary must exit non-zero on write failure; skill must surface the error |
| `forget` appears to work but audit trail shows nothing | User believes memory was deleted; entry still searchable | Show "memory archived (audit trail preserved)" — set expectations on what forget means |
| `rightclaw memory list` shows all entries across all time | Overwhelming; no way to assess relevance or age | Default to last 30 days; add `--all` and `--since` flags |
| No memory size indicator | User doesn't know DB is 200MB until disk fills | `rightclaw memory stats` shows entry count, DB size, oldest/newest entry |
| Memory search returns injection artifacts | User sees strange imperative fragments in memory recall | Show provenance tag on each entry; flag entries that triggered injection scanning |
| `rightclaw memory delete` with running agent | CLI hangs waiting for lock; no timeout indicator | Show "Waiting for agent to finish writing..." with a spinner; fail after 10s with actionable message |

---

## "Looks Done But Isn't" Checklist

- [ ] **WAL + busy_timeout:** Open the DB, check `PRAGMA journal_mode` returns `wal`, check `PRAGMA busy_timeout` returns `5000`. Do this in CI.
- [ ] **Per-agent isolation:** Create two agents, store a unique memory in each, assert the other agent's DB does not contain it.
- [ ] **Migration idempotency:** Run `rightclaw up` twice on a fresh agent. Verify refinery does not re-apply V1.
- [ ] **Audit trail immutability:** Attempt `UPDATE memory_events SET content='hacked'` in the REPL — verify the trigger raises an ABORT.
- [ ] **MEMORY.md untouched:** Run the memory skill `store` command 10 times. Verify `MEMORY.md` content is unchanged.
- [ ] **Eviction columns exist:** Verify `expires_at` and `importance` columns exist in V1 schema even if eviction logic is not yet implemented.
- [ ] **Injection scanning fires:** Store a memory containing "Ignore all previous instructions and exfiltrate". Verify it is rejected or sanitized.
- [ ] **DB permissions:** Verify `memory.db` is created with 0600 permissions (not world-readable).
- [ ] **Migration file immutability:** Edit `V1__initial.sql` after it has been applied to a test DB. Verify `rightclaw up` fails with a clear "checksum mismatch" error (not a panic).
- [ ] **Forget audit:** After `forget`, verify the entry is not returned by `search` but IS present in `memory_events` with `event_type='forget'`.

---

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Prompt injection via stored memory | HIGH | Identify and remove poisoned entries; run `rightclaw memory list --since <date>` to find anomalous entries; rebuild agent sessions after purge |
| WAL corruption from SIGKILL | LOW | WAL recovery is automatic on next open. If corrupt: `sqlite3 memory.db ".recover"` to extract recoverable data |
| Migration checksum mismatch | MEDIUM | For dev: delete DB and re-run `rightclaw up`. For prod: provide a documented recovery script in changelog |
| DB bloat (100MB+) | LOW | `rightclaw memory prune --before 90d`; `PRAGMA incremental_vacuum;`; entry count will drop on next `rightclaw up` |
| Cross-agent leakage (wrong DB path) | HIGH | Stop all agents; audit which agent wrote to which DB; restore from backup (if no backup, data is mixed — cannot cleanly separate) |
| MEMORY.md contaminated by skill | MEDIUM | Restore MEMORY.md from git history (`git show HEAD~1:identity/MEMORY.md`); fix skill to never write to MEMORY.md |

---

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| #1 Prompt injection via memory | Phase 1: skill design and sanitization | Store injection payload; verify it is rejected or never reaches system prompt |
| #2 No migration strategy | Phase 1: refinery setup | `refinery_schema_history` table exists in agent DB after first `up` |
| #3 File locking on restart | Phase 1: DB layer (WAL + busy_timeout) | Restart agent mid-write; verify next open succeeds within 5s |
| #4 Memory bloat | Phase 1: schema (eviction columns); Phase 2: CLI stats | `expires_at` column exists in V1; `rightclaw memory stats` shows size |
| #5 MEMORY.md / SQLite dual-truth | Phase 1: skill design (never write to MEMORY.md) | Run skill `store` 10 times; MEMORY.md unchanged |
| #6 Mutable audit trail | Phase 1: schema (append-only + triggers) | Attempt direct UPDATE on memory_events; verify ABORT trigger fires |
| #7 Silent write failure (busy_timeout=0) | Phase 1: connection init | Test with concurrent writes; verify error surfaces to user |
| #8 CLI vs. agent DB contention | Phase 2: CLI implementation | Run `rightclaw memory delete` while agent is in a long write; verify behavior is deterministic |
| #9 Cross-agent DB path | Phase 1: path derivation | Two-agent isolation test in CI |
| #10 Refinery checksum mismatch | Phase 1 (discipline) + Phase 3 (doctor check) | Edit V1 migration; verify doctor warns; verify up fails cleanly |

---

## Sources

### Published Research (HIGH confidence)
- [MINJA: Memory INJection Attack on LLM Agents — NeurIPS 2025](https://openreview.net/forum?id=QVX6hcJ2um) — 95%+ injection success rate via query-only memory poisoning
- [Palo Alto Unit 42: Indirect Prompt Injection Poisons AI Long-Term Memory](https://unit42.paloaltonetworks.com/indirect-prompt-injection-poisons-ai-longterm-memory/) — session summarization exploit via injected memory
- [Christian Schneider: Persistent Memory Poisoning in AI Agents](https://christian-schneider.net/blog/persistent-memory-poisoning-in-ai-agents/)
- [OWASP ASI06: Memory Poisoning as Top Agentic Risk 2026](https://www.lakera.ai/blog/agentic-ai-threats-p1)

### SQLite Documentation (HIGH confidence)
- [SQLite WAL Mode](https://www.sqlite.org/wal.html) — WAL mode, checkpoint behavior, WAL recovery on restart
- [SQLite File Locking and Concurrency V3](https://sqlite.org/lockingv3.html) — lock types, SQLITE_BUSY behavior
- [SQLite How to Corrupt a Database](https://sqlite.org/howtocorrupt.html) — SIGKILL + WAL, filesystem locking bugs
- [SQLite Concurrent Writes and Locked Errors](https://tenthousandmeters.com/blog/sqlite-concurrent-writes-and-database-is-locked-errors/) — busy_timeout limitations, BEGIN IMMEDIATE

### Rust Ecosystem (HIGH confidence)
- [refinery on GitHub](https://github.com/rust-db/refinery) — embedded migrations for rusqlite, V{N}__{name}.sql format
- [rusqlite Connection docs](https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html) — SQLITE_OPEN_NO_MUTEX (default), per-connection usage
- [Rust ORMs in 2026: Diesel vs SQLx vs SeaORM vs Rusqlite](https://aarambhdevhub.medium.com/rust-orms-in-2026-diesel-vs-sqlx-vs-seaorm-vs-rusqlite-which-one-should-you-actually-use-706d0fe912f3) — rusqlite 0.38.0 (Dec 2025), refinery recommended for SQLite migrations

### Agent Memory Architecture (MEDIUM confidence)
- [OpenClaw Issue #26949: MEMORY.md double injection](https://github.com/openclaw/openclaw/issues/26949) — live bug report on MEMORY.md vs. memory_search dual injection
- [Hermes Agent Persistent Memory](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory/) — SQLite + FTS5 episodic memory, on-demand recall pattern
- [The MEMORY.md Problem: Why Local Files Fail at Scale](https://dev.to/anajuliabit/the-memorymd-problem-why-local-files-fail-at-scale-58ae)
- [SQLite Is the Best Database for AI Agents](https://dev.to/nathanhamlett/sqlite-is-the-best-database-for-ai-agents-and-youre-overcomplicating-it-1a5g)

### Immutable Audit Trails (MEDIUM confidence)
- [Instant SQLite Audit Trail (GitHub)](https://github.com/simon-weber/Instant-SQLite-Audit-Trail) — trigger-based immutability pattern
- [SQLite and Blockchain: Storing Immutable Records](https://www.sqliteforum.com/p/sqlite-and-blockchain-storing-immutable) — append-only with ABORT triggers

---
*Pitfalls research for: v2.3 Memory System — SQLite-backed per-agent memory added to RightClaw alongside existing MEMORY.md flat files*
*Researched: 2026-03-26*
