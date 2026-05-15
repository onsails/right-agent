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

| Caller | Module | mode | Schema | Model |
|--------|--------|------|--------|-------|
| Worker (Telegram messages) | `telegram/worker.rs` | `Normal` or `Bootstrap` | reply-schema.json / bootstrap-schema.json | agent config |
| Cron (scheduled jobs) | `cron.rs` | `Cron` | CRON_SCHEMA_JSON | agent config |
| Cron (background continuation) | `cron.rs` (`ScheduleKind::BackgroundContinuation`) | `Cron` | BG_CONTINUATION_SCHEMA_JSON | agent config |
| Delivery (cron result relay) | `cron_delivery.rs` | `Normal` | reply-schema.json | claude-haiku-4-5-20251001 |
| Reflection (post-failure summary) | `reflection.rs` | `Normal` | reply-schema.json | agent config |

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

**Debug args.** When `agent.yaml::debug` (hot-reloadable via the `/debug`
Telegram command) is true, `ClaudeInvocation` also appends
`--debug --debug-file=/sandbox/.claude/logs/<session-uuid>.log`. The
session UUID matches CC's own JSONL filename. Off by default.

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
{composite-memory: bot-trusted system note (label) → ironclaw
 untrusted-content wrap (SECURITY NOTICE + BEGIN/END EXTERNAL CONTENT,
 with boundary escape) → optional bot-trusted status / bg markers.
 Wrap text is owned by `ironclaw_safety::wrap_external_content` and
 may evolve with crate updates; see `docs/architecture/memory.md` for
 the integration.}
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
`Healthy`. Four states:

- `degraded — recall may be incomplete or stale, retain may be queued` —
  circuit breaker is open or half-open, or a recent transient failure occurred.
- `unavailable — Hindsight Cloud account is out of credits. Memory ops will
  fail until the user tops up. IMPORTANT: tell the user clearly that they
  need to add credits at https://hindsight.vectorize.io to restore memory.` —
  HTTP 402 from Hindsight (insufficient credits). Sticky until any 2xx clears
  it (e.g., the first call after the user tops up).
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

### Cron mode

```
[Base: Right Agent agent description, sandbox info, MCP reference]

## Operating Instructions
{compiled-in from templates/right/prompt/OPERATING_INSTRUCTIONS.md}

## Cron Delivery Contract
{compiled-in from templates/right/prompt/CRON_INSTRUCTIONS.md}

## Your Identity
{IDENTITY.md}

## Your Personality and Values
{SOUL.md}

## Your User
{USER.md}

## Environment and Tools
{TOOLS.md}

## MCP Server Instructions  (if any external MCP servers have instructions)
{fetched from aggregator via POST /mcp-instructions}
```

Cron mode is selected by `cron::execute_job` for both regular cron
runs (`CRON_SCHEMA_JSON`) and background-continuation runs
(`BG_CONTINUATION_SCHEMA_JSON`). The memory section is intentionally
omitted — cron jobs are static instructions, not user queries; agents
that need memory call `memory_recall` explicitly from the cron prompt.

The `## Cron Delivery Contract` block tells the agent that its
structured output is the Telegram delivery channel and that the turn
has no live user. See [issue #48](https://github.com/onsails/right-agent/issues/48)
for the production incidents that motivated this section.
The operating instructions also define the cron idle UX rule: results are
auto-delivered only after the chat has been idle for 2 minutes, and the
agent must never promise delivery sooner than 2 minutes.

### Compiled-in Content

Operating instructions, cron-delivery contract, and bootstrap content
are compiled into the binary via `include_str!()` from
`templates/right/prompt/` and `templates/right/agent/`.
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
| skills/ | `/sandbox/.claude/skills/right-mcp` → `/platform/skills/right-mcp.<hash>` | Platform (symlink) |
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
Optional: `reply_to_message_id`, `attachments`, `used_skill_receipts`,
`learning_signal`, `skill_issue_signal`.

**Attachments.** Each item in `attachments` accepts an optional `media_group_id`
(nullable string). Items sharing the same value are delivered as a single
Telegram media group (album). Validation and degradation rules match Telegram's
`sendMediaGroup` constraints — see `### Media Groups (Albums)` in
`OPERATING_INSTRUCTIONS.md` for the full rules shown to the agent.

**Learned-skill metadata.** `used_skill_receipts` is an optional nullable array
of `{ package_name, message }`; receipt messages are appended to the Telegram
reply. `learning_signal` is an optional nullable `create_candidate` object for
candidate skill creation, and `skill_issue_signal` is an optional nullable
`update_candidate` object for candidate skill updates. Both signals require
non-empty `event_refs` and enum-constrained reason/type fields; the bot may
drop ambiguous or low-evidence signals without affecting reply delivery.

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
cron (list/show runs), MCP management (add/remove/list/auth), foreground
progress (mcp__right__send_progress), and bootstrap
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

`mcp__right__send_progress` is available only for foreground Telegram
invocations. It sends a separate Telegram message (max 2000 characters), is
rate limited to one message every 30 seconds per invocation, and returns
tool-level errors such as `progress_unavailable`, `progress_forbidden`,
`progress_rate_limited`, or `progress_send_failed`. Cron, delivery, reflection,
and background-continuation turns deny this tool via `--disallowedTools`.

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
