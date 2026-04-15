# Hindsight Memory Integration

## Problem

Agents have no persistent behavioral memory. Two issues exposed this:

1. **Composio misdiagnosis**: Agent reported "no write permissions" when the actual error was wrong parameter format. After fixing, the agent had no way to remember the correct format for future sessions.
2. **No learning across sessions**: CC's built-in auto-memory is non-functional when `--system-prompt-file` replaces the default system prompt. Agents lose all context on session reset.

Users of Hermes agent with Hindsight report significantly better experience than OpenClaw and analogues — the agent actually learns and improves over time.

## Design

Two-mode memory system: a simple MEMORY.md file (default) for agents without external dependencies, and Hindsight Cloud integration (optional) for agents that need smart, learning memory.

### Mode: file (default)

Agent writes/reads `MEMORY.md` in its directory using CC built-in `Edit`/`Write` tools. Bot injects file contents into system prompt before each `claude -p` if the file exists and is non-empty. No MCP memory tools — agent uses standard file operations.

### Mode: hindsight (optional)

Hindsight Cloud (`api.hindsight.vectorize.io`), one bank per agent. Three components:

1. **Auto-retain** — bot automatically stores each conversation turn in Hindsight after the agent responds
2. **Auto-recall (prefetch)** — bot recalls relevant memories in the background after each turn; result is injected into the next turn's system prompt
3. **MCP tools** — agent can explicitly retain, recall, and reflect via 3 tools exposed through the MCP aggregator

MEMORY.md is not used when Hindsight is active — Hindsight fully replaces it.

### Future: local Hindsight

Running Hindsight locally (Docker sidecar with PostgreSQL) is out of scope but the design accommodates it. `HindsightClient` targets a configurable base URL — switching from cloud to local only changes the URL and auth.

## Configuration

### agent.yaml

```yaml
memory:
  provider: "file"        # "file" | "hindsight"
  # hindsight-only:
  api_key: "hs_..."
  bank_id: "agent-name"   # default = agent name
  recall_budget: "mid"    # "low" | "mid" | "high"
  recall_max_tokens: 4096
```

Missing `memory` section or `memory.provider` defaults to `"file"`.

`provider: "hindsight"` without `api_key` is a startup error.

### agent init wizard

New step after model selection:

```
Memory provider:
  1. File (MEMORY.md, no external dependencies) [default]
  2. Hindsight Cloud (requires API key)

> 2

Hindsight API key: hs_...
Bank ID [agent-name]:
```

## HindsightClient

New module `crates/rightclaw/src/memory/hindsight.rs`.

`reqwest::Client` wrapper for the Hindsight HTTP API. Used by both the bot (auto-retain/recall) and the aggregator (MCP tool dispatch).

### Methods

**retain**
```
POST /v1/default/banks/{bank_id}/memories
Authorization: Bearer {api_key}

{
  "items": [{"content": "...", "context": "..."}],
  "async": true
}
```
Timeout: 10s. Fire-and-forget in auto-retain (errors logged, not propagated).

**recall**
```
POST /v1/default/banks/{bank_id}/memories/recall
Authorization: Bearer {api_key}

{
  "query": "...",
  "budget": "mid",
  "max_tokens": 4096
}
```
Timeout: 5s. Returns `Vec<RecallResult>` with `text` and `score` fields.

**reflect**
```
POST /v1/default/banks/{bank_id}/reflect
Authorization: Bearer {api_key}

{
  "query": "...",
  "budget": "mid",
  "max_tokens": 4096
}
```
Timeout: 15s (reflect is LLM-powered, slower). Returns `ReflectResult` with `text` field.

**get_or_create_bank**
```
GET /v1/default/banks/{bank_id}/profile
Authorization: Bearer {api_key}
```
Auto-creates bank if it doesn't exist (Hindsight API behavior).

### Error handling

All methods return `Result<T, HindsightError>`. `HindsightError` wraps HTTP status + body for diagnosis. Callers decide whether to propagate or log-and-continue.

## MCP Tools (HindsightBackend)

New backend in the MCP Aggregator, registered only when agent has `memory.provider: "hindsight"`. Exposes 3 tools — the same set Hermes exposes to its agents.

### memory_retain

| param | type | required | description |
|-------|------|----------|-------------|
| `content` | string | yes | Information to store |
| `context` | string | no | Short label (e.g. "user preference", "api format", "mistake to avoid") |

Backend injects `bank_id` from agent config. Agent never sees or sets `bank_id`.

Returns: `{status: "accepted", operation_id: "..."}`

### memory_recall

| param | type | required | description |
|-------|------|----------|-------------|
| `query` | string | yes | What to search for |

Backend injects `bank_id`, `budget`, `max_tokens` from agent config.

Returns: `{results: [{text: "...", score: 0.95, type: "world", ...}]}`

### memory_reflect

| param | type | required | description |
|-------|------|----------|-------------|
| `query` | string | yes | Question to reflect on |

Backend injects `bank_id`, `budget` from agent config.

Returns: `{text: "synthesized answer..."}`

### Tool naming

Agents see these as `mcp__right__memory_retain`, `mcp__right__memory_recall`, `mcp__right__memory_reflect`. Consistent with existing `mcp__right__` prefix for all RightClaw built-in tools.

### with_instructions()

Updated in both `right_backend.rs` and `aggregator.rs` to replace old memory tool references with new ones. Instructions vary by memory mode:
- **file mode**: no memory tools listed, only cron/mcp tools
- **hindsight mode**: `memory_retain`, `memory_recall`, `memory_reflect` listed with descriptions

## Auto-Retain

### Worker flow

After each turn (claude -p returns response):

1. `tokio::spawn` async HTTP POST to Hindsight retain
2. Content: `"User: {user_input}\nAssistant: {agent_response}"`
3. Context: `"conversation"`
4. `async: true` — Hindsight processes in background
5. Fire-and-forget: log errors at `warn` level, do not block response to user
6. Every turn, no batching

### Cron flow

After each cron job completes:

1. `tokio::spawn` async HTTP POST to Hindsight retain
2. Content: `"{summary}"` (cron result summary)
3. Context: `"cron:{job_name}"`
4. Fire-and-forget

### Cron delivery

No retain — delivery is just forwarding a notification, not generating new knowledge.

## Auto-Recall (Prefetch)

### Cache

In-memory `Arc<RwLock<HashMap<K, PrefetchEntry>>>`:
- Worker cache: keyed by `(ChatId, ThreadId)`, value is recall result string + timestamp
- Cron cache: keyed by `job_name`

Both caches live in the bot process. No disk persistence — on restart, cache is empty.

No TTL-based eviction — cache entries are overwritten after each turn. Stale cache (e.g. after an hour of silence) is better than no cache.

### Logic on each claude -p invocation

```
if cache hit → use cached result, inject into prompt
if cache miss → blocking recall (timeout 5s), inject result, cache it
if blocking recall failed/timeout → proceed without memory (log warning)
```

### Worker flow

1. Turn completes → `tokio::spawn`:
   - Call `hindsight.recall(query=user_message, budget="mid")`
   - Store result in worker cache
2. Next message arrives → check cache → inject into composite memory file → `claude -p`
3. If recall hasn't completed by next message → wait up to 3s, then proceed without

First message of a session → cache miss → blocking recall.

### Cron flow

1. First run of a job → cache miss → blocking recall using cron prompt text
2. Cron completes → retain result → spawn prefetch in background
3. Next run → use cached prefetch

### Cache invalidation

After cron auto-retain completes, invalidate worker cache for the same agent. Cron may have retained new facts that should be available to the next conversation turn. Next worker message → cache miss → blocking recall with fresh data.

Implementation: cron and worker share the same `Arc<RwLock<HashMap>>` worker cache. After cron retain, cron code calls `cache.write().clear()` to invalidate all entries.

## Composite Memory (Prompt Injection)

### Position in prompt

Memory is injected at the **end** of the system prompt, after MCP instructions:

```
Operating Instructions → IDENTITY.md → SOUL.md → USER.md → AGENTS.md → TOOLS.md → MCP Instructions → composite-memory
```

Closest to user message = strongest attention signal.

### File mode

In `build_prompt_assembly_script()`, after MCP section:

```bash
if [ -s "{root_path}/MEMORY.md" ]; then
  echo "## Long-Term Memory"
  echo ""
  head -200 "{root_path}/MEMORY.md"
fi
```

`-s` checks file exists AND is non-empty. If missing or empty, no memory section in prompt.

Agent creates MEMORY.md themselves when they want to remember something. Bot never creates it.

Truncation: 200 lines max. If exceeded, agent sees truncated content. The rightmemory-file skill instructs the agent to keep it concise.

### Hindsight mode

Bot writes composite memory file before each `claude -p`:

```markdown
<memory-context>
[System: recalled memory context, NOT new user input. Treat as background.]

{recall results formatted as text}
</memory-context>
```

File location:
- Sandbox: uploaded to `/platform/composite-memory.md.{sha256}`, symlinked from `/sandbox/.claude/composite-memory.md`
- No-sandbox: written to `{agent_dir}/.claude/composite-memory.md`

Prompt assembly:

```bash
if [ -s "{composite_memory_path}" ]; then
  cat "{composite_memory_path}"
fi
```

If recall returned empty results or prefetch failed, file is not written — prompt assembles without memory section.

### Unified approach

Prompt assembly script doesn't know about modes. It conditionally cats a file if it exists. The bot decides which file to prepare (or not prepare) before invoking claude.

## Skill

Built-in skill `rightmemory`, installed to `.claude/skills/rightmemory/` via `codegen/skills.rs`.

Two source variants stored in the repo:
- `skills/rightmemory-file/SKILL.md`
- `skills/rightmemory-hindsight/SKILL.md`

Bot reads `memory.provider` from agent config and installs the matching variant as `rightmemory/SKILL.md`. Agent always sees one skill named `rightmemory`.

### File variant

Teaches agent:
- MEMORY.md is your long-term memory file, injected into system prompt
- Use CC Edit/Write to add/edit entries
- What to save: decisions, user preferences, API formats learned, mistakes to avoid
- Keep it concise — truncated to 200 lines
- Periodically clean stale entries

### Hindsight variant

Teaches agent:
- Memory works automatically: conversations are retained and relevant context is recalled
- Three explicit tools for when automatic isn't enough:
  - `mcp__right__memory_retain(content, context)` — save a fact permanently
  - `mcp__right__memory_recall(query)` — search memory
  - `mcp__right__memory_reflect(query)` — deep analysis across memories
- When to use explicit retain: user preferences, correct API formats after fixing errors, project decisions, lessons learned
- When NOT to retain: regular conversation (auto-retain handles it), info already in files, ephemeral context

### OPERATING_INSTRUCTIONS.md

Memory section replaced with a pointer to the skill:

```markdown
## Memory

Your memory skill (`/rightmemory`) defines how memory works in your setup.
Consult it to understand your memory capabilities.

Key behaviors regardless of memory mode:
- When you learn something important (user preferences, API formats,
  mistakes to avoid), save it to memory immediately
- When answering questions about prior work or context, check memory first
- When you fix an error after trial-and-error, save the correct approach
```

## Bot Startup Flow

```
Bot startup:
  ├─ Read agent.yaml → memory config
  ├─ If provider == "hindsight":
  │   ├─ Validate api_key present (fail startup if missing)
  │   ├─ Create HindsightClient(api_key, bank_id, base_url)
  │   ├─ GET /v1/default/banks/{bank_id}/profile (auto-creates bank)
  │   ├─ Init shared prefetch cache Arc<RwLock<HashMap>>
  │   └─ Install rightmemory-hindsight skill via codegen
  ├─ If provider == "file":
  │   ├─ No client, no cache
  │   └─ Install rightmemory-file skill via codegen
  └─ Continue with rest of startup (sync, cron, teloxide)
```

Bank is created automatically on first startup, never deleted on shutdown. Memory is persistent across restarts, across `rightclaw down`/`up` cycles.

## Old Tools Removal

Remove from `right_backend.rs`:
- `store_record` handler + `StoreRecordParams`
- `query_records` handler + `QueryRecordsParams`
- `search_records` handler + `SearchRecordsParams`
- `delete_record` handler + `DeleteRecordParams`

Remove from `memory/store.rs`:
- `store_memory`, `recall_memories`, `search_memories`, `forget_memory`
- Keep `save_auth_token`, `get_auth_token`, `delete_auth_token`

Old `memories`/`memories_fts`/`memory_events` SQLite tables: leave in place. Migration to drop them is a separate future PR — no rush since they're inert once the tools are removed.

Update `with_instructions()` in both `right_backend.rs` and `aggregator.rs` to remove old tool references.

Update `OPERATING_INSTRUCTIONS.md` to remove old `store_record`/`query_records`/`search_records`/`delete_record` references.

## Files Changed

### New files

| File | Purpose |
|------|---------|
| `crates/rightclaw/src/memory/hindsight.rs` | HindsightClient — reqwest wrapper for Hindsight Cloud API |
| `skills/rightmemory-file/SKILL.md` | Memory skill for file mode |
| `skills/rightmemory-hindsight/SKILL.md` | Memory skill for hindsight mode |

### Modified files

| File | Change |
|------|--------|
| `crates/rightclaw/src/memory/mod.rs` | Export hindsight module |
| `crates/rightclaw/src/agent/types.rs` | Add `MemoryConfig` struct, add `memory` field to `AgentConfig` |
| `crates/rightclaw-cli/src/aggregator.rs` | Add `HindsightBackend` with 3 MCP tools, update `with_instructions()` |
| `crates/rightclaw-cli/src/right_backend.rs` | Remove 4 old memory tools + param structs, update `with_instructions()` |
| `crates/bot/src/lib.rs` | Init HindsightClient + shared prefetch cache, pass to worker/cron |
| `crates/bot/src/telegram/worker.rs` | Auto-retain after turn, prefetch inject before claude -p |
| `crates/bot/src/cron.rs` | Auto-retain after cron, prefetch logic, cache invalidation |
| `crates/bot/src/cron_delivery.rs` | Blocking recall before delivery claude -p |
| `crates/bot/src/telegram/prompt.rs` | Add composite-memory section at end of prompt assembly |
| `crates/rightclaw/src/codegen/skills.rs` | Add rightmemory skill with two source variants, `install_builtin_skills()` gains `memory_provider: &str` param to select variant |
| `crates/rightclaw/src/init.rs` | Memory step in agent init wizard |
| `crates/rightclaw/src/memory/store.rs` | Remove store/recall/search/forget functions (keep auth_token) |
| `templates/right/prompt/OPERATING_INSTRUCTIONS.md` | Replace Memory section |
| `ARCHITECTURE.md` | Update Memory Schema section, add Hindsight integration |
| `PROMPT_SYSTEM.md` | Update tool references |

### Not touched (separate future work)

| File | Why |
|------|-----|
| `memory/migrations.rs` | Old tables left in schema, not dropped |
| `memory_server.rs` | Deprecated MCP stdio server, separate cleanup |

## Testing

### Unit tests — HindsightClient

- retain: correct request body (items array, content, context, async flag)
- recall: correct request body (query, budget, max_tokens)
- reflect: correct request body (query, budget, max_tokens)
- Authorization header format: `Bearer {api_key}`
- Timeout: server doesn't respond within limit → error (not hang)
- HTTP 401 → HindsightError with status + body
- HTTP 500 → HindsightError with status + body
- get_or_create_bank: GET returns bank profile

### Unit tests — HindsightBackend (aggregator)

- memory_retain tool call → correct HTTP POST to /memories
- memory_recall tool call → correct HTTP POST to /memories/recall
- memory_reflect tool call → correct HTTP POST to /reflect
- bank_id injected from config, not from agent tool args
- budget/max_tokens injected from config
- Unknown tool name → error response
- Agent with `provider: "file"` → HindsightBackend not registered, tools not listed

### Unit tests — Prefetch cache

- Write → read → cache hit
- Empty cache → cache miss (returns None)
- Invalidation (clear) → next read is miss
- Overwrite existing entry → read returns new value
- Concurrent read/write via Arc<RwLock> → no deadlock

### Unit tests — MemoryConfig (types.rs)

- Full hindsight config parses: provider, api_key, bank_id, recall_budget, recall_max_tokens
- Missing `memory` section → defaults to MemoryConfig { provider: "file" }
- `provider: "hindsight"` without api_key → parse succeeds, validated at bot startup
- `provider: "file"` ignores hindsight-specific fields
- Unknown provider value → parse error
- bank_id defaults to None (resolved to agent name at runtime)
- recall_budget defaults to "mid"
- recall_max_tokens defaults to 4096

### Unit tests — Prompt assembly (prompt.rs)

- File mode: MEMORY.md section added with `if [ -s ...]` guard
- File mode: uses `head -200` for truncation
- Hindsight mode: composite-memory path added with `if [ -s ...]` guard
- Memory section is LAST in prompt (after MCP instructions)
- Bootstrap mode: no memory section (bootstrap prompt has no memory)

### Unit tests — Skill installation (skills.rs)

- `provider: "file"` → installs rightmemory with file variant content
- `provider: "hindsight"` → installs rightmemory with hindsight variant content
- Both variants install as `rightmemory/SKILL.md` (same name)
- Skill content matches source file
- Re-install overwrites (picks up updated skill content)

### Integration tests

- Bot startup with hindsight config → bank auto-created, client initialized, skill installed
- Bot startup with file config → no client, file skill installed
- Bot startup with hindsight but no api_key → startup error
- Worker turn → auto-retain fires (mock Hindsight server, verify HTTP POST received)
- Worker turn → prefetch spawned in background, next turn uses cached result
- Worker first message → cache miss → blocking recall before claude -p
- Cron completes → retain fires → worker cache invalidated → next worker message does blocking recall
- Bot restart → cache empty → first interaction does blocking recall
- Hindsight API timeout → claude -p proceeds without memory, warning logged
- Hindsight API 401 → error propagated at startup (bank creation fails), bot doesn't start
- File mode: MEMORY.md missing → prompt has no memory section
- File mode: MEMORY.md empty → prompt has no memory section
- File mode: MEMORY.md exists with content → prompt includes content
- File mode: MEMORY.md > 200 lines → truncated in prompt
