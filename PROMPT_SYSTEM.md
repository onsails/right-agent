# Right Agent Prompting System

How Right Agent constructs the prompt for each `claude -p` invocation.

## Composite System Prompt Architecture

Every CC invocation gets a **single composite system prompt** assembled from multiple files.
No `--agent` flag — all content is in `--system-prompt-file`.

**Why not `--agent`?** Testing proved that `--agent` with `@` file references doesn't work
reliably when MCP tools are present (~8K+ tokens of tool definitions drown the agent's
instructions). The model cross-validates `@`-injected content against the filesystem and
ignores it when files aren't at the working directory.

**Why `--system-prompt-file`?** It replaces CC's default system prompt entirely, giving our
instructions highest priority.

**Prompt caching is critical.** Avoid approaches that cause per-message tool calls to read
identity files — this breaks CC's prompt caching and adds latency.

## Prompt Assembly

A single function `build_prompt_assembly_script()` in `telegram/prompt.rs` generates a
parameterized shell script that assembles the composite prompt. The script is
identical for both modes — only the `root_path` parameter differs:

- **Sandbox mode (OpenShell):** `root_path=/sandbox`, executed via SSH
- **No-sandbox mode:** `root_path=agent_dir`, executed via `bash -c`

The script `cat`s compiled-in content and agent-owned files at `root_path`,
producing the composite prompt in microseconds. Files are always fresh (no sync delay).

### Callers

All three CC invocation paths use `build_prompt_assembly_script()`:

| Caller | Module | bootstrap_mode | Schema | Model |
|--------|--------|---------------|--------|-------|
| Worker (Telegram messages) | `telegram/worker.rs` | true/false | reply-schema.json | agent config |
| Cron (scheduled jobs) | `cron.rs` | false | CRON_SCHEMA_JSON | agent config |
| Cron (background continuation) | `cron.rs` (`ScheduleKind::BackgroundContinuation`) | false | BG_CONTINUATION_SCHEMA_JSON | agent config |
| Delivery (cron result relay) | `cron_delivery.rs` | false | reply-schema.json | claude-haiku-4-5-20251001 |

`cron::execute_job` selects between `CRON_SCHEMA_JSON` and
`BG_CONTINUATION_SCHEMA_JSON` via `select_schema_and_fork` (in
`crates/bot/src/cron.rs`): the `BackgroundContinuation { fork_from }`
variant routes to `BG_CONTINUATION_SCHEMA_JSON` and supplies
`fork_from` as the `--resume`/`--fork-session` source; all other kinds
use `CRON_SCHEMA_JSON` with no fork.

**Model selection.** The agent's Claude model is read from
`agent.yaml::model` (or omitted for CC's default). Users can switch via
the Telegram `/model` command, which writes to `agent.yaml` and hot-reloads
without restart — the next CC invocation passes `--model <new>`.

## Prompt Structure

### Normal mode

```
[Base: Right Agent agent description, sandbox info, MCP reference]

## Operating Instructions
{compiled-in from templates/right/prompt/OPERATING_INSTRUCTIONS.md}

## Your Identity
{IDENTITY.md — name, creature, vibe, emoji, principles}

## Your Personality and Values
{SOUL.md — core values, communication style, boundaries}

## Your User
{USER.md — user name, timezone, preferences}

## Environment and Tools
{TOOLS.md — agent-owned tools and environment notes}

## MCP Server Instructions  (if any external MCP servers have instructions)
{fetched from aggregator via POST /mcp-instructions at prompt assembly time}

## Memory
{composite-memory — file mode: MEMORY.md contents truncated to 200 lines;
 Hindsight mode: prefetched recall results injected as context}
```

Missing agent-owned files are silently skipped. Operating instructions and bootstrap
content are compiled into the binary — no file sync needed. MCP instructions are
fetched from the aggregator's internal API (non-fatal if unavailable). Memory section
is appended last: file mode inlines MEMORY.md contents, Hindsight mode inlines
prefetched recall results.

### Memory Status Marker

When the agent runs with `memory.provider: hindsight`, the bot injects a
`<memory-status>...</memory-status>` marker at the end of
`composite-memory.md` whenever the ResilientHindsight wrapper is not
`Healthy`. Three states:

- `degraded — recall may be incomplete or stale, retain may be queued` —
  circuit breaker is open or half-open, or a recent transient failure occurred.
- `unavailable — memory provider authentication failed, memory ops will error
  until the user rotates the API key` — 401/403 from Hindsight. Requires
  user action.
- `retain-errors: N records dropped in last 24h due to bad payload — check
  logs` — in a Healthy state but Client-kind (4xx) retain drops occurred in
  the last 24h.

The marker is always the last section of the system prompt, preserving
prompt cache for all preceding blocks.

### Bootstrap mode

```
[Base: Right Agent agent description, sandbox info, MCP reference]

## Bootstrap Instructions
{compiled-in from templates/right/agent/BOOTSTRAP.md}
```

### Compiled-in Content

Operating instructions and bootstrap content are compiled into the binary via
`include_str!()` from `templates/right/prompt/` and `templates/right/agent/`.
Changes to these files take effect on `cargo build` + restart — no file sync needed.
This eliminates the stale-template problem where changes to platform instructions
required manual re-init of existing agents.

## Base Prompt

Generated by `generate_system_prompt()` in `codegen/agent_def.rs`.
Content: agent name, Right Agent description, sandbox mode, home/working directory, MCP reference, repo link.

### SSH Awareness Block (Openshell Sandbox Only)

When an agent runs with `sandbox: mode: openshell`, the base prompt includes a "## User SSH Access" section:

```
## User SSH Access

If an operation requires an interactive terminal (TUI, interactive prompts,
password input) that you cannot perform from within your sandbox — tell the
user to run:

  right agent ssh <name>
  right agent ssh <name> -- <command>

Examples:
- `gh auth login`
- `gcloud auth login`
- `npm login`
- Any command with interactive prompts or TUI

Always provide the exact command with the `--` separator when passing a specific command.
```

This block instructs the agent to suggest SSH access for operations requiring interactive shells.
Agents with `sandbox: mode: none` (no sandbox, direct host access) do NOT include this block.

## File Locations

### Sandbox

Agent-owned files live at `/sandbox/` root. Platform-managed files live in `/platform/`
(content-addressed, read-only) and are symlinked from their expected paths.

| File | Path | Owner |
|------|------|-------|
| IDENTITY.md | `/sandbox/IDENTITY.md` | Agent (bootstrap) |
| SOUL.md | `/sandbox/SOUL.md` | Agent (bootstrap) |
| USER.md | `/sandbox/USER.md` | Agent (bootstrap) |
| TOOLS.md | `/sandbox/TOOLS.md` | Agent (editable) |
| settings.json | `/sandbox/.claude/settings.json` → `/platform/settings.json.<hash>` | Platform (symlink) |
| reply-schema.json | `/sandbox/.claude/reply-schema.json` → `/platform/...` | Platform (symlink) |
| skills/ | `/sandbox/.claude/skills/rightmcp` → `/platform/skills/rightmcp.<hash>` | Platform (symlink) |
| BOOTSTRAP.md | N/A (not synced to sandbox) | Content from compiled-in constant; on-disk file is host-side flag only |

### Host (`agent_dir/`)

| File | Path | Synced by |
|------|------|----------|
| IDENTITY.md | `agent_dir/IDENTITY.md` | reverse_sync |
| SOUL.md | `agent_dir/SOUL.md` | reverse_sync |
| USER.md | `agent_dir/USER.md` | reverse_sync |
| TOOLS.md | `agent_dir/TOOLS.md` | reverse_sync |
| BOOTSTRAP.md | `agent_dir/BOOTSTRAP.md` | template (deleted after bootstrap) |

## JSON Schemas

### reply-schema.json (normal mode)
Required: `content` (string|null).
Optional: `reply_to_message_id`, `attachments`.

**Attachments.** Each item in `attachments` accepts an optional `media_group_id`
(nullable string). Items sharing the same value are delivered as a single
Telegram media group (album). Validation and degradation rules match Telegram's
`sendMediaGroup` constraints — see `### Media Groups (Albums)` in
`OPERATING_INSTRUCTIONS.md` for the full rules shown to the agent.

### bootstrap-schema.json (bootstrap mode)
Same as reply-schema plus required `bootstrap_complete` (boolean).
Server-side validation: `bootstrap_complete: true` is ignored unless IDENTITY.md,
SOUL.md, USER.md all exist on the host after reverse_sync.

### CRON_SCHEMA_JSON (cron jobs — default)
Defined in `crates/right-agent/src/codegen/agent_def.rs`. Required:
`summary` (string). Optional: `notify` (object | null) and
`no_notify_reason` (string | null). When `notify` is non-null, its
`content` field is required. `notify: null` is the silent-output path
(cron ran but has nothing to report); `no_notify_reason` should then
carry a short factual explanation.

### BG_CONTINUATION_SCHEMA_JSON (cron jobs — background continuation)
Defined in `crates/right-agent/src/codegen/agent_def.rs`. Selected by
`cron::execute_job` via `select_schema_and_fork` for
`ScheduleKind::BackgroundContinuation` runs (foreground turns the
worker offloaded to a forked session). Differs from `CRON_SCHEMA_JSON`:

- `notify` is REQUIRED and non-null — silent output is forbidden because
  the user is waiting for the foreground answer that was sent to
  background.
- `notify.content` has `minLength: 1` (no empty replies).
- `no_notify_reason` is absent from the schema — silence is not a valid
  outcome for this job kind.

`summary` remains required for log/analytics parity with
`CRON_SCHEMA_JSON`.

## MCP Server Instructions

The `right` MCP server provides `with_instructions()` describing all tools:
memory (memory_retain/memory_recall/memory_reflect — Hindsight mode only),
cron (list/show runs), MCP management (add/remove/list/auth), and bootstrap
(mcp__right__bootstrap_done).

Update `with_instructions()` in both `memory_server.rs` and `aggregator.rs`
whenever tools change.

### Error Convention

Tool failures return `is_error: true` with a JSON body of shape

    { "error": { "code": "<code>", "message": "<human readable>", "details"?: {...} } }

Operation errors are normal and recoverable; the agent reads `error.code` to
decide whether to retry, surface to the user, or take a different path.
Protocol errors (JSON-RPC errors) indicate a bug in the agent's tool call
itself (unknown tool, missing/malformed argument).

Cross-cutting codes any tool may emit: `upstream_unreachable`, `upstream_auth`,
`upstream_invalid`, `circuit_open`, `invalid_argument`, `tool_failed`,
`server_not_found`. Tool-specific codes are listed in each tool's
description.

## Upstream MCP Server Instructions

When external MCP servers are registered (via `/mcp add`), their usage instructions are
fetched from the aggregator's internal API (`POST /mcp-instructions`) at prompt assembly
time and inlined into the composite system prompt. This replaces the previous file-based
approach (MCP_INSTRUCTIONS.md).

Instructions are persisted in SQLite (`mcp_servers.instructions` column) by ProxyBackend
on each `connect()`. The endpoint reads from SQLite via `db_list_servers()` and generates
markdown via `generate_mcp_instructions_md()`.

### ⟨⟨SYSTEM_NOTICE⟩⟩ Markers

When the platform needs to inject a platform-level message into the agent's
conversation (currently: only error reflection after a CC invocation failure),
it wraps the injected text in `⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`. The
agent is taught via `OPERATING_INSTRUCTIONS` ("System Notices" section) that
such messages are not from the user and should be acted on for the current
turn but not treated as user input on subsequent turns.

The reflection primitive lives at `crates/bot/src/reflection.rs`. See
ARCHITECTURE.md § "Reflection Primitive" for lifecycle details.

## Bootstrap Completion Flow

1. Agent sends response with `bootstrap_complete: true` in structured output
2. Worker runs blocking `reverse_sync_md` (pulls files from sandbox to host)
3. `should_accept_bootstrap()` checks IDENTITY.md + SOUL.md + USER.md on host
4. If all present → delete session, delete BOOTSTRAP.md → normal mode
5. If missing → ignore bootstrap_complete, continue bootstrap mode

Bootstrap instructions explicitly tell the agent to write files in CWD (not `.claude/agents/`).

Additionally, `mcp__right__bootstrap_done` MCP tool provides in-session feedback: agent calls it
after creating files, gets immediate success/error response with missing file list.
