# Self-Introspection: `/rightreflect` skill + `/debug` command

**Date:** 2026-05-12
**Status:** Design — pending plan
**Author:** brainstorm session, recorded by Claude

## Problem

The agent has no built-in way to read its own past reasoning when the user
asks "why did you ...?" The thinking, tool calls, and tool results are all
written to disk by Claude Code, but the agent doesn't know where, doesn't
have a vocabulary for finding the right file, and can't tell whether deeper
debug logs exist.

The trigger anecdote: a one-shot reminder fired on the wrong day. The user
asked the agent why. The agent succeeded only because the operator was
already running with `--debug` (informally) and could point the agent at
the right files. We want this to be a first-class capability, not an
accident of operator configuration.

## Goals

1. The agent always knows how and where to read its own past reasoning.
2. The mechanism is filesystem-only — no new MCP tools, no host→sandbox
   sync, no DB queries from the sandbox.
3. Deeper analysis (raw API/MCP transport noise) is available on demand
   via a runtime toggle, without restarting the bot or losing context.
4. When deeper analysis would help but isn't currently available, the
   agent can clearly tell the user how to enable it.

## Non-goals

1. No new MCP tools. Discovery uses the agent's existing Read/Bash/Grep.
2. No host→sandbox sync of `~/.right/logs/streams/<uuid>.ndjson`. The
   agent reads CC's project JSONL inside the sandbox instead.
3. No `claude -p --debug` in production by default. Off unless toggled.
4. No category filtering on `--debug`. Bare `--debug` only.
5. No log retention/rotation/cleanup for `/sandbox/.claude/logs/`. Files
   accumulate; addressed only if disk pressure shows up.
6. No automatic retry/replay machinery for `/debug on`. Flipping the
   toggle does not retroactively re-run any session or cron.
7. No DB schema changes.
8. No agent-facing exposure of host-side logs.
9. No frontmatter or system-prompt section on self-reflection. All
   knowledge lives in the skill; OPERATING_INSTRUCTIONS.md gets one
   pointer line.
10. No work on the wrong-day-reminder bug itself. Separate issue.

## Background — what already exists

- **`/sandbox/.claude/projects/-sandbox/<session-uuid>.jsonl`** — written
  by Claude Code on every turn. One JSON object per line. Contains
  `assistant` events (with `thinking`, `tool_use` content blocks),
  `user` events (with `tool_result`), `system` events (init/shutdown
  with tool list and MCP servers), `rate_limit_event`, `result`. The
  filename matches the `--session-id` we pass to `claude`. Already
  exists in every sandbox today; no enabling flag needed. Verified
  2026-05-12 against `rightclaw-test` sandbox.
- **`~/.right/logs/streams/<uuid>.ndjson`** (host) — the bot's own
  capture of CC's stream-json output. Same conceptual content as the
  JSONL above, raw. Host-side; not reachable from sandbox.
- **CC `--debug` and `--debug-file <path>`** — supported by `claude` CLI
  but not currently passed by Right Agent. Without `--debug`, no
  separate debug log is written anywhere.
- **`right bot --debug`** today only adds `--verbose` to CC and bumps
  the bot's tracing level. It does NOT pass `--debug` to claude.
- **`mcp__right__cron_trigger`** — existing MCP tool the agent can use
  to retrigger a cron run.
- **`/model` Telegram command + `Arc<ArcSwap<Option<String>>>` +
  `config_watcher::diff_classify`** — existing pattern for
  hot-reloadable agent settings.

## Architecture

Three changes, in order of size:

### 1. New bundled skill `/rightreflect`

Path: `crates/right-codegen/skills/rightreflect/SKILL.md`. Bundled via
`include_dir!` into the binary, deployed by `right_codegen::skills` to
`<agent_dir>/.claude/skills/rightreflect/` on every codegen pass
(`Regenerated(BotRestart)` category).

The skill teaches the agent:

1. **When to activate** — user asks "why did you ...", "what were you
   thinking when ...", reflection on a wrong action; or the agent itself
   wants to reconcile two conflicting prior decisions before answering.
2. **Where the data lives**:
   - Primary: `/sandbox/.claude/projects/-sandbox/<session-uuid>.jsonl`.
   - Fallback (only if `/debug` is on): `/sandbox/.claude/logs/<session-uuid>.log`.
3. **Finding the right file**:
   - Current session: agent uses its own session UUID. (Resolved at
     skill runtime: TBD — see Open Questions.)
   - By topic/keyword: `grep -l "<keyword>" /sandbox/.claude/projects/-sandbox/*.jsonl | xargs ls -lt`.
   - By date window: `ls -lt /sandbox/.claude/projects/-sandbox/`.
4. **Reading efficiently** — files run from 8 KB to 33 MB. Never `cat`.
   Use `jq -c 'select(.type=="assistant" and (.message.content[]?.type == "thinking"))'`,
   etc. Skill ships 2-3 example one-liners.
5. **Interpreting** — small table of the five top-level event types
   (`assistant`, `user`, `system`, `rate_limit_event`, `result`) and
   what each tells the agent. Plus the content-block structure
   (`thinking.text`, `tool_use.name + .input`, `tool_result.content`).
6. **Reporting back** — narrative format with file paths and line
   numbers; brevity rule (1-3 relevant turns, not a transcript dump).
7. **Fallback decision tree**:
   - JSONL covered it → done.
   - JSONL didn't cover it, debug logs exist for the session → read them.
   - JSONL didn't cover it, no debug logs → tell user: *"For deeper
     analysis I'd need API/transport-level logs, which require debug
     mode. Send `/debug on` in this chat and ask me again — future
     turns will produce them."*
8. **Cron-run reflection branch**: if reflecting on a cron run (not a
   conversational session) and deeper logs would help, the skill teaches
   the agent to consider replay. Three options, in order of caution:
   - Ask the user first when the cron has non-idempotent side effects
     (sent a message, posted to an external API, modified files,
     charged money).
   - Self-trigger silently if clearly safe to repeat (pure read, pure
     analysis, idempotent fetch). Mechanism: `mcp__right__cron_trigger`.
   - Sequence guard: if `/debug` is off, don't trigger first — the
     rerun would produce no debug logs and waste the action.
   - Hard limits: never retrigger more than once per analysis; always
     check the cron's prompt text for side-effect-y language before
     deciding.
9. **Important rules**:
   - Never modify any file under `/sandbox/.claude/projects/`.
   - Never echo a full thinking block verbatim — summarize.
   - Schema is Anthropic-controlled — fall back to `grep`/`head` on raw
     lines if `jq` selectors return nothing, and report the surprise.

### 2. `claude -p --debug --debug-file=...` when toggle is on

`crates/bot/src/cc/invocation.rs` — `ClaudeInvocation` reads a shared
`Arc<AtomicBool>` at `build_args()` time (not at construction). When
`true`:

```text
--debug
--debug-file=/sandbox/.claude/logs/<session-uuid>.log
```

`<session-uuid>` is the same UUID already passed via `--session-id`. The
directory `/sandbox/.claude/logs/` is created by CC on first write — no
upload step.

### 3. Hot-reloadable `debug` flag + `/debug` Telegram command

#### `agent.yaml` schema

New optional field:

```yaml
debug: false   # optional; default false
```

CLI flag `right bot --debug` provides the initial value when yaml is
unset. Once `/debug on` writes to yaml, yaml wins on restart.

#### Wiring

- `BotSettings.debug: Arc<AtomicBool>` — initial value
  `agent.yaml::debug.unwrap_or(args.debug)`.
- `ClaudeInvocation` reads `.load(Ordering::Relaxed)` per build.
- `config_watcher::diff_classify` — `ChangeKind::HotReloadable {
  new_model, new_debug }`. Both fields nulled before equality check.
- Watcher applies `new_debug` to the AtomicBool when classified as
  hot-reloadable.

In-flight CC processes keep their old flags; the next invocation in any
chat picks up the new value. No process-compose restart, no agent
context loss.

#### `/debug` Telegram command

Handler under `crates/bot/src/telegram/`, registered alongside `/model`,
`/mcp`. Persists state through `agent.yaml`.

| Command | Effect |
|---|---|
| `/debug` | Reports current state. If on: also `ls /sandbox/.claude/logs/<sid>.log` for current session and reports presence/size. If off: explains what `on` would enable. |
| `/debug on` | Sets `agent.yaml::debug: true`. Watcher picks up within debounce window (~250ms). Reply: "Debug mode on. Future turns will write API/transport logs to `/sandbox/.claude/logs/<session>.log`. Past turns are unchanged." |
| `/debug off` | Sets `agent.yaml::debug: false`. Reply: "Debug mode off. Existing logs remain." |

`/debug` does NOT delete existing log files — neither on `off` nor on
restart. Cleanup is out of scope.

## Data flow — agent self-introspection turn

1. User asks "why did you set the reminder for Wednesday?"
2. Agent activates `/rightreflect`.
3. Agent finds the relevant JSONL: `grep -l "reminder" /sandbox/.claude/projects/-sandbox/*.jsonl`, picks the most-recent hit.
4. Agent extracts thinking + tool_use turns near the keyword.
5. Agent reports a narrative: "At 09:14 my thinking said the right day was Tuesday. Three turns later I called `cron_create` with `2026-05-13` (Wednesday). Then in a different session I checked the spec and reassured myself it was correct."
6. If the JSONL doesn't explain *why* the slip happened (often the case
   for reasoning errors): agent checks `/sandbox/.claude/logs/<sid>.log`.
   If absent → recommends `/debug on` for next time.

## File-change catalog

**New files:**
- `crates/right-codegen/skills/rightreflect/SKILL.md`

**Modified Rust files:**
- `crates/right-codegen/src/skills.rs` — register `SKILL_RIGHTREFLECT`,
  add to install array.
- `crates/right-agent/src/agent/types.rs` — add `pub debug: Option<bool>`
  to `AgentConfig`.
- `crates/right-agent/templates/right/agent/agent.yaml` — add commented-out
  `# debug: false` line for discoverability.
- `crates/bot/src/lib.rs` — `BotSettings.debug: Arc<AtomicBool>`. Initial
  value `agent.yaml::debug.unwrap_or(args.debug)`.
- `crates/bot/src/config_watcher.rs` — extend
  `ChangeKind::HotReloadable { new_model, new_debug }`, null both fields
  in `diff_classify`. New tests.
- `crates/bot/src/cc/invocation.rs` — read AtomicBool at build_args time;
  emit `--debug --debug-file=/sandbox/.claude/logs/<sid>.log` when true.
- `crates/bot/src/telegram/dispatch.rs` (or sibling) — register `/debug`
  command filter.
- `crates/bot/src/telegram/handler.rs` (or new
  `crates/bot/src/telegram/debug_command.rs`) — handler.
- `/help` text site (TBD location) — add `/debug` entry.

**Modified docs:**
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
  — add one line under Core Skills: `- /rightreflect — read your own
  past sessions when asked "why did you..."`. Add brief mention of
  `/debug` next to other Telegram commands the user controls.
- `ARCHITECTURE.md` — extend "Hot-reloadable fields in agent.yaml" to
  list `debug` alongside `model`. Add `/sandbox/.claude/logs/` to the
  runtime-paths section.
- `PROMPT_SYSTEM.md` — note that `--debug --debug-file=...` are
  conditionally appended when `/debug` is on.
- `docs/architecture/sessions.md` — note the JSONL location is
  agent-readable for self-introspection (cite `/rightreflect`).

**No changes to:**
- `right-mcp` aggregator (no new MCP tools).
- `right-codegen/src/process_compose.rs` (existing `--debug` flag still
  works for fresh boots).
- DB schema.
- Host stream NDJSON writers.
- OpenShell policy.

## Tests

**Unit:**
1. `config_watcher::diff_classify`:
   - `diff_debug_only_is_hot_reloadable`
   - `diff_debug_and_model_combined_is_hot_reloadable`
   - `diff_debug_plus_other_field_is_restart`
   - `diff_no_debug_field_set_treats_as_false`
2. `ClaudeInvocation`:
   - `debug_atomic_bool_true_appends_debug_and_debug_file`
   - `debug_atomic_bool_false_omits_them`
   - `debug_file_path_uses_session_id`
   - `debug_value_read_at_build_time_not_construction_time`
3. `right-codegen::skills`:
   - Existing install test gains `rightreflect/SKILL.md` assertion.
   - New test: SKILL.md frontmatter parses as YAML with `name: rightreflect`.
4. Telegram `/debug` handler:
   - `debug_on_writes_yaml_field`
   - `debug_off_writes_false`
   - `debug_status_with_logs_present_reports_size`
   - `debug_status_with_no_logs_explains_off_state`

**Integration (live OpenShell sandbox via `TestSandbox::create(...)`):**
5. `cc_debug_file_lands_inside_sandbox` — invoke `claude -p` with
   `--debug --debug-file=/sandbox/.claude/logs/<sid>.log`; verify file
   exists and contains a stable substring discoverable in the same test.
6. `jsonl_file_exists_for_invoked_session` — verify
   `/sandbox/.claude/projects/-sandbox/<sid>.jsonl` exists after a CC
   invocation. Sanity check on the assumption that holds the whole
   skill up.
7. `skill_can_grep_jsonl` — write a synthetic JSONL with a marker, run
   the skill's grep pattern via `sandbox.exec()`, assert the file is
   found.

**Not tested (deliberately):**
- The skill's *judgment* about retriggering crons (prose; reviewable
  manually).
- E2E "user types `/debug on` in real Telegram, agent writes a debug
  file" — too much fixture surface for too little marginal coverage.

**Manual verification** (in the plan, for whoever executes):
- [ ] Restart bot fresh; `/debug` reports off.
- [ ] `/debug on`; ask agent something trivial; verify
  `/sandbox/.claude/logs/<sid>.log` exists with non-zero size.
- [ ] Ask agent "why did you ..." about a past turn; verify it activates
  `/rightreflect`, finds the right JSONL, and reports a coherent
  narrative.
- [ ] `/debug off`; verify next turn does NOT write a new debug log.

## Open questions

1. **How does the agent learn its own current session UUID** without
   reading env vars or files? Three options to evaluate during plan:
   (a) inject the UUID into the system prompt at assembly time, (b) the
   skill teaches the agent to read `/proc/self/cmdline` (CC was launched
   with `--session-id <uuid>`), (c) the skill tells the agent it can
   simply look at the most-recently-modified JSONL in
   `/sandbox/.claude/projects/-sandbox/`. Default to (c) — cheapest,
   no codegen change. Resolve in plan.
2. **Skill name.** `/rightreflect` is the working name. Alternatives:
   `/rightthoughts`, `/rightself`, `/rightintrospect`, `/rightreplay`.
   Pick during plan.
3. **`/help` text site.** Need to confirm exact location of the
   command-list rendering (likely `crates/bot/src/telegram/handler.rs`
   or similar). Resolve in plan.
4. **Precedence semantics for CLI flag vs yaml field.** Stated
   intention: yaml wins on restart once explicitly set. Need to be
   precise about how "explicitly set" is detected (presence of the key
   vs `Some(false)`). Probably `Option<bool>` semantics: `None` means
   "not set, fall back to CLI"; `Some(_)` means "yaml owns it".

## Known risks (acknowledged, not addressed)

- CC's project JSONL schema is Anthropic-controlled and may change.
  Skill instructs agent to fall back to grep/head if `jq` selectors
  return nothing.
- `--debug` log files may grow unboundedly per session. No size cap, no
  rotation. Sandboxes are persistent and never deleted, so this could
  matter eventually.
- The agent may misjudge a cron's idempotency and self-trigger
  destructively. Mitigated by hard rules in the skill (check prompt
  text first, ask user when uncertain), but not eliminated.

## References

- `ARCHITECTURE.md` — Configuration Hierarchy, Hot-reloadable fields,
  Claude Invocation Contract.
- `PROMPT_SYSTEM.md` — composite system prompt assembly.
- `crates/bot/src/cc/invocation.rs` — invocation builder.
- `crates/bot/src/config_watcher.rs` — `diff_classify`, hot-reload
  pattern.
- `crates/right-codegen/src/skills.rs` — bundled skills installer.
- `crates/right-codegen/skills/rightcron/SKILL.md`,
  `crates/right-codegen/skills/rightmemory-*/SKILL.md` — reference
  skills for tone, structure, length.
