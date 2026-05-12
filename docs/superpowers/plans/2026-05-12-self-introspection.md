# Self-Introspection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the agent a first-class skill for reading its own past reasoning, plus a hot-reloadable `/debug` Telegram command that surfaces deeper API/transport logs on demand.

**Architecture:** Three additive changes — (1) bundled `/rightreflect` skill teaching the agent to read CC's project JSONL inside the sandbox, (2) `claude -p --debug --debug-file=...` appended when a hot-reloadable debug toggle is on, (3) `/debug [on|off|status]` Telegram command persisting via `agent.yaml::debug` and hot-reloading through the existing `config_watcher` pattern. No new MCP tools, no DB migrations, no host→sandbox sync.

**Tech Stack:** Rust 2024, tokio, teloxide (Telegram), arc_swap, notify, include_dir, serde_saphyr (YAML), thiserror, anyhow, miette. Tests: `cargo test --workspace`. Live integration via `right_core::test_support::TestSandbox` against OpenShell.

**Spec:** [`docs/superpowers/specs/2026-05-12-self-introspection-design.md`](../specs/2026-05-12-self-introspection-design.md)

---

## File Map

**New:**
- `crates/right-codegen/skills/rightreflect/SKILL.md` — the skill itself (Task 1).
- `crates/bot/src/telegram/debug_command.rs` — `/debug` handler (Task 9).

**Modified:**
- `crates/right-core/src/agent_types.rs` — `AgentConfig::debug: Option<bool>`.
- `crates/right-agent/src/agent/types.rs` — `write_agent_yaml_debug` line writer.
- `crates/right-agent/templates/right/agent/agent.yaml` — commented `# debug: false`.
- `crates/bot/src/telegram/handler.rs` — `AgentSettings::debug` becomes `Arc<AtomicBool>`.
- `crates/bot/src/lib.rs` — construct the `Arc<AtomicBool>`, plumb into watcher and settings, add `snapshot_debug` helper.
- `crates/bot/src/config_watcher.rs` — `ChangeKind::HotReloadable { new_model, new_debug }`, null both before equality check, watcher applies `new_debug`.
- `crates/bot/src/cc/invocation.rs` — read `Arc<AtomicBool>` at build time, emit `--debug --debug-file=/sandbox/.claude/logs/<sid>.log`.
- `crates/bot/src/telegram/mod.rs` — register `debug_command` module.
- `crates/bot/src/telegram/dispatch.rs` — register `BotCommand::Debug(...)` and route to handler.
- `crates/right-codegen/src/skills.rs` — register `SKILL_RIGHTREFLECT`, add to install array.
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md` — Core Skills entry + `/debug` mention.
- `ARCHITECTURE.md`, `PROMPT_SYSTEM.md`, `docs/architecture/sessions.md` — small additions.

---

## Task 1: Create the `/rightreflect` skill content

This is the user-visible artifact. We start here so subsequent tasks can refer back to its exact text.

**Files:**
- Create: `crates/right-codegen/skills/rightreflect/SKILL.md`

- [ ] **Step 1: Create the skill file**

```bash
mkdir -p crates/right-codegen/skills/rightreflect
```

Then write `crates/right-codegen/skills/rightreflect/SKILL.md`:

````markdown
---
name: rightreflect
description: >-
  Inspects this agent's own past reasoning by reading the conversation-history
  JSONL files Claude Code writes inside the sandbox. Use when the user asks
  "why did you...", "what were you thinking when...", or to debug a wrong
  decision the agent made in an earlier turn or session. Reads thinking
  blocks, tool calls, and tool results to reconstruct prior reasoning.
  Filesystem-only — no MCP calls, no DB.
---

# /rightreflect — Read Your Own Past Reasoning

You are reflecting on your own past sessions to answer "why did you ...?"
The data exists. Your job is to find the right file, read it efficiently,
and report a coherent narrative.

## When to Activate

- The user asks "why did you ...?", "what were you thinking when ...?",
  "you got X wrong, what happened?"
- You yourself notice two of your own past decisions disagree and want
  to reconcile them before answering.
- The user is debugging something that involved a past cron run or a
  past conversational turn.

## Where Your Reasoning Is Stored

**Primary — always available:**

```
/sandbox/.claude/projects/-sandbox/<session-uuid>.jsonl
```

One JSON object per line. Contains every `thinking` block, every
`tool_use`, every `tool_result`, plus session lifecycle events. This is
Claude Code's own conversation-history file — it exists for every
session, no flag required.

**Fallback — only if `/debug` is on:**

```
/sandbox/.claude/logs/<session-uuid>.log
```

Raw API request/response payloads, MCP transport events, hook firings.
Verbose. Only present when the user has enabled `/debug on`. Check with:

```bash
ls /sandbox/.claude/logs/ 2>/dev/null
```

If empty or "No such file or directory" — you are not in debug mode.
See `Fallback Decision Tree` below for what to tell the user.

## Finding the Right File

Three patterns, in order of preference:

### 1. The most recent session

Almost always the answer when the user is asking about something that
just happened in this conversation:

```bash
ls -lt /sandbox/.claude/projects/-sandbox/*.jsonl | head -3
```

The most recent file is your current or most-recent session.

### 2. By topic / keyword

When the user asks about something that happened "yesterday" or "in the
session about X":

```bash
grep -l "<keyword>" /sandbox/.claude/projects/-sandbox/*.jsonl | xargs -I{} ls -lt {} | head -5
```

Pick the most-recent hit. The keyword can be a cron job name, a tool
name, a topic word from the conversation. Cron job names show up
verbatim in `tool_use.input` for `mcp__right__cron_create`,
`mcp__right__cron_trigger`, etc.

### 3. By date window

When you know roughly when something happened:

```bash
ls -lt /sandbox/.claude/projects/-sandbox/ | head -20
```

Files are sorted by mtime. Pick by date.

## Reading Efficiently

**Files run from 8 KB to 33 MB.** Never `cat` the whole file. Use `jq`
to extract structured slices.

### Just my thinking blocks

```bash
jq -c 'select(.type=="assistant") | .message.content[]? | select(.type=="thinking") | .text' \
  /sandbox/.claude/projects/-sandbox/<sid>.jsonl
```

### Just my tool calls

```bash
jq -c 'select(.type=="assistant") | .message.content[]? | select(.type=="tool_use") | {name, input}' \
  /sandbox/.claude/projects/-sandbox/<sid>.jsonl
```

### Tool results I received

```bash
jq -c 'select(.type=="user") | .message.content[]? | select(.type=="tool_result") | {tool_use_id, content}' \
  /sandbox/.claude/projects/-sandbox/<sid>.jsonl
```

### Combine: a turn-by-turn narrative

```bash
jq -c 'select(.type=="assistant" or .type=="user") | {ts: .timestamp, type, text: (.message.content[0].text // .message.content[0].input // .message.content[0].content)}' \
  /sandbox/.claude/projects/-sandbox/<sid>.jsonl | head -50
```

### Schema escape hatch

If `jq` selectors return nothing — Anthropic may have changed the JSONL
schema. Fall back to raw line inspection:

```bash
head -1 /sandbox/.claude/projects/-sandbox/<sid>.jsonl | jq .
```

Inspect the actual structure, then adjust your selector. Report the
schema surprise to the user.

## Interpreting the Events

| Top-level `type` | What it tells you |
|---|---|
| `assistant` | Your own reply for one turn. `message.content[]` has `thinking` (your reasoning), `tool_use` (tool calls you made), and `text` (what you sent the user). |
| `user` | Either the human's message or a `tool_result` for one of your prior `tool_use` calls. The `tool_use_id` ties the result back to the call. |
| `system` | Session lifecycle. `subtype: init` lists every tool and MCP server you had at the start. `subtype: shutdown` is end-of-session. |
| `result` | Per-invocation summary. Contains `total_cost_usd`, token counts, `stop_reason`. Useful for "did I run out of budget?" |
| `rate_limit_event` | Throttling. Useful when reasoning was rushed. |

## Reporting Back

Format: a tight narrative with file paths and line numbers, so the user
can verify. Brevity rule — 1 to 3 turns, never a transcript dump.

**Good:**

> At line 47 my thinking said the right day was Tuesday 2026-05-12. Three
> turns later (line 58) I called `mcp__right__cron_create` with
> `schedule: "0 9 13 5 *"` — Wednesday — and the call succeeded. I never
> reread my own thinking before committing to that schedule.
>
> File: `/sandbox/.claude/projects/-sandbox/<sid>.jsonl`

**Bad:**

> Here's the entire transcript: [3000 lines]

Never echo a full thinking block verbatim — summarize. Thinking blocks
contain raw reasoning that may be inappropriate to surface unfiltered.

## Fallback Decision Tree

After reading the JSONL:

1. **JSONL covered it.** Report. Done.
2. **JSONL didn't cover it, debug logs exist for the session.** Read
   `/sandbox/.claude/logs/<sid>.log`. Use `grep` for `error`, `warn`,
   `MCP`, or specific tool names. Same brevity rule applies.
3. **JSONL didn't cover it, no debug logs for this session.** Tell the
   user verbatim:
   > "For deeper analysis I'd need API/transport-level logs, which
   > require debug mode. Send `/debug on` in this chat and ask me again
   > — future turns will produce them. (Past turns are unchanged.)"

## Cron-Run Reflection — Replay Option

When the analysis target is a **cron run** (not a conversational
session) and the JSONL alone doesn't explain the behavior, you may
consider replaying the cron to collect debug logs from a fresh run.

**Sequence guard:** if `/debug` is currently off, do NOT trigger first.
The rerun would produce no debug logs and waste the action. First
ensure `/debug on` is in effect (or ask the user to flip it).

**Three options, in order of caution:**

1. **Ask the user first** when the cron's real-world side effects are
   non-idempotent. Read the cron's `prompt` text via `mcp__right__cron_list`
   first; if it contains side-effect language ("send", "post", "create",
   "delete", "pay", "publish"), or calls non-idempotent tools, ASK before
   triggering. Suggested phrasing:

   > "To collect deeper logs I'd need to retrigger this cron, which would
   > [describe side effect] again. Want me to: (a) retrigger now with
   > debug, (b) wait — you turn on `/debug on` and I'll trigger when you
   > say go, or (c) skip — I'll work with what I have."

2. **Self-trigger silently** if the cron action is *clearly safe to
   repeat* — pure read, pure analysis, idempotent fetch with no
   stateful effects. Mechanism:

   ```
   mcp__right__cron_trigger { "job_name": "<name>" }
   ```

3. **Hard limits** — never retrigger more than once per analysis. If a
   single retrigger does not produce useful logs, report what was tried
   and stop.

## Important Rules

1. **Read-only.** Never modify any file under
   `/sandbox/.claude/projects/`. These are CC's source of truth for
   `--resume`. Editing them corrupts session history.
2. **Summarize thinking blocks; never echo verbatim.** Internal
   reasoning text may not be safe to surface to the user as-is.
3. **Schema may evolve.** The JSONL format is Anthropic-controlled. If
   `jq` returns nothing, fall back to raw `head -1 | jq .` and report
   what you saw.
4. **One reflection per question.** If you cannot find a clear answer
   in the JSONL + (optionally) the debug log, say so directly. Do not
   guess. Do not retrigger crons multiple times.
````

- [ ] **Step 2: Verify the file exists and is readable**

```bash
test -f crates/right-codegen/skills/rightreflect/SKILL.md && head -5 crates/right-codegen/skills/rightreflect/SKILL.md
```

Expected: prints the YAML frontmatter opening.

- [ ] **Step 3: Commit**

```bash
git add crates/right-codegen/skills/rightreflect/SKILL.md
git commit -m "feat(skills): add /rightreflect skill content for self-introspection"
```

---

## Task 2: Add `debug` field to `AgentConfig`

The yaml schema must accept `debug: bool` before any other code can read it. TDD: failing test first.

**Files:**
- Modify: `crates/right-core/src/agent_types.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing `mod tests` in `crates/right-core/src/agent_types.rs`:

```rust
    #[test]
    fn agent_config_debug_field_defaults_to_none() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.debug, None);
    }

    #[test]
    fn agent_config_debug_true_parses() {
        let yaml = "debug: true";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.debug, Some(true));
    }

    #[test]
    fn agent_config_debug_false_parses() {
        let yaml = "debug: false";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.debug, Some(false));
    }
```

(`mod tests` for `AgentConfig` lives near the top of `agent_types.rs`.
If there is no existing test module there, add `#[cfg(test)] mod tests {
use super::*; ... }` at the end of the file. Verify by running
`grep -n 'mod tests' crates/right-core/src/agent_types.rs` first.)

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p right-core agent_config_debug -- --nocapture
```

Expected: 3 failures with "no field `debug` on type `AgentConfig`" or similar.

- [ ] **Step 3: Add the field to `AgentConfig`**

In `crates/right-core/src/agent_types.rs`, find `pub struct AgentConfig {` (around line 195). Add the new field after `pub model: Option<String>,` (line 210):

```rust
    /// When `Some`, controls whether `claude -p` runs with --debug --debug-file=...
    /// Hot-reloadable via `/debug` Telegram command. `None` falls back to the
    /// `right bot --debug` CLI flag at boot time.
    #[serde(default)]
    pub debug: Option<bool>,
```

Then update `impl Default for AgentConfig` to include `debug: None,` after `model: None,`:

```rust
            model: None,
            debug: None,
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p right-core agent_config_debug -- --nocapture
```

Expected: 3 passes.

- [ ] **Step 5: Run the existing `agent_config_rejects_unknown_fields` test to confirm it still works**

```bash
cargo test -p right-core agent_config_rejects_unknown_fields -- --nocapture
```

Expected: pass. The struct still uses `deny_unknown_fields`, so adding a known field does not break the rejection of unknown ones.

- [ ] **Step 6: Commit**

```bash
git add crates/right-core/src/agent_types.rs
git commit -m "feat(core): add agent.yaml::debug optional field"
```

---

## Task 3: Add `write_agent_yaml_debug` line writer

Mirrors `write_agent_yaml_model`. The `/debug` Telegram command will use this to persist `debug: true|false` without touching unrelated yaml fields, comments, or blank lines.

**Files:**
- Modify: `crates/right-agent/src/agent/types.rs`

- [ ] **Step 1: Write the failing tests**

Append to the existing `mod tests` in `crates/right-agent/src/agent/types.rs`:

```rust
    #[test]
    fn write_agent_yaml_debug_appends_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "restart: never\nmax_restarts: 5\n").unwrap();

        super::write_agent_yaml_debug(&path, Some(true)).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("restart: never"), "preserve existing fields:\n{result}");
        assert!(result.contains("max_restarts: 5"), "preserve existing fields:\n{result}");
        assert!(result.contains("debug: true"), "append debug when absent:\n{result}");
        let parsed: AgentConfig = serde_saphyr::from_str(&result).unwrap();
        assert_eq!(parsed.debug, Some(true));
    }

    #[test]
    fn write_agent_yaml_debug_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "restart: never\ndebug: false\nmax_restarts: 5\n").unwrap();

        super::write_agent_yaml_debug(&path, Some(true)).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(!result.contains("debug: false"), "old value gone:\n{result}");
        assert!(result.contains("debug: true"), "new value present:\n{result}");
        let restart_pos = result.find("restart:").unwrap();
        let debug_pos = result.find("debug:").unwrap();
        assert!(restart_pos < debug_pos, "field order preserved:\n{result}");
    }

    #[test]
    fn write_agent_yaml_debug_removes_when_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "restart: never\ndebug: true\nmax_restarts: 5\n").unwrap();

        super::write_agent_yaml_debug(&path, None).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(!result.contains("debug:"), "debug line removed:\n{result}");
        assert!(result.contains("restart: never"));
        let parsed: AgentConfig = serde_saphyr::from_str(&result).unwrap();
        assert!(parsed.debug.is_none());
    }

    #[test]
    fn write_agent_yaml_debug_preserves_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.yaml");
        std::fs::write(&path, "# header\nrestart: never\n\n# comment\nmax_restarts: 5\n").unwrap();

        super::write_agent_yaml_debug(&path, Some(true)).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("# header"));
        assert!(result.contains("# comment"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p right-agent write_agent_yaml_debug -- --nocapture
```

Expected: 4 failures, `cannot find function 'write_agent_yaml_debug' in module 'super'`.

- [ ] **Step 3: Add the function**

In `crates/right-agent/src/agent/types.rs`, after the existing
`write_agent_yaml_model` function (ends around line 64):

```rust
/// Write `agent.yaml::debug` via line-oriented MergedRMW.
///
/// `Some(true|false)` replaces or appends a `debug: <value>` line.
/// `None` removes the existing `debug:` line.
///
/// Preserves all unknown fields, comments, and blank lines via
/// `right_codegen::contract::write_merged_rmw`. Same line-walking pattern
/// as `write_agent_yaml_model`.
pub fn write_agent_yaml_debug(
    path: &std::path::Path,
    new_value: Option<bool>,
) -> miette::Result<()> {
    right_codegen::contract::write_merged_rmw(path, |existing| {
        let original = existing.unwrap_or("");

        let mut found = false;
        let mut out = String::with_capacity(original.len() + 32);
        for line in original.split_inclusive('\n') {
            let is_top_level_debug = line
                .strip_prefix("debug:")
                .map(|rest| {
                    rest.starts_with(' ')
                        || rest.starts_with('\t')
                        || rest.is_empty()
                        || rest.starts_with('\n')
                        || rest.starts_with('\r')
                })
                .unwrap_or(false);
            if is_top_level_debug {
                found = true;
                if let Some(v) = new_value {
                    let needs_newline = line.ends_with('\n');
                    out.push_str(&format!("debug: {v}{}", if needs_newline { "\n" } else { "" }));
                }
            } else {
                out.push_str(line);
            }
        }

        if !found && let Some(v) = new_value {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&format!("debug: {v}\n"));
        }

        Ok(out)
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p right-agent write_agent_yaml_debug -- --nocapture
```

Expected: 4 passes.

- [ ] **Step 5: Commit**

```bash
git add crates/right-agent/src/agent/types.rs
git commit -m "feat(right-agent): write_agent_yaml_debug line writer for /debug command"
```

---

## Task 4: Extend `config_watcher::diff_classify` for hot-reloadable debug

`ChangeKind::HotReloadable` must also carry the new debug value so the watcher can update both `model_arc` and the new `debug_arc` atomically. TDD with the new test cases first.

**Files:**
- Modify: `crates/bot/src/config_watcher.rs`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/bot/src/config_watcher.rs`:

```rust
    #[test]
    fn diff_debug_only_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\ndebug: false\n";
        let new = "restart: never\nmax_restarts: 5\ndebug: true\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model, new_debug } => {
                assert!(new_model.is_none());
                assert_eq!(new_debug, Some(true));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_debug_added_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\n";
        let new = "restart: never\nmax_restarts: 5\ndebug: true\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model, new_debug } => {
                assert!(new_model.is_none());
                assert_eq!(new_debug, Some(true));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_debug_removed_is_hot_reloadable() {
        let old = "restart: never\nmax_restarts: 5\ndebug: true\n";
        let new = "restart: never\nmax_restarts: 5\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model, new_debug } => {
                assert!(new_model.is_none());
                assert!(new_debug.is_none());
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_debug_and_model_combined_is_hot_reloadable() {
        let old = "restart: never\nmodel: \"claude-sonnet-4-6\"\ndebug: false\n";
        let new = "restart: never\nmodel: \"claude-haiku-4-5\"\ndebug: true\n";
        match classify(old, new) {
            ChangeKind::HotReloadable { new_model, new_debug } => {
                assert_eq!(new_model.as_deref(), Some("claude-haiku-4-5"));
                assert_eq!(new_debug, Some(true));
            }
            other => panic!("expected HotReloadable, got {other:?}"),
        }
    }

    #[test]
    fn diff_debug_plus_other_field_is_restart_required() {
        let old = "restart: never\nmax_restarts: 5\ndebug: false\n";
        let new = "restart: always\nmax_restarts: 5\ndebug: true\n";
        assert!(matches!(classify(old, new), ChangeKind::RestartRequired));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p right-bot diff_debug -- --nocapture
```

Expected: 5 failures with destructure errors (`HotReloadable` doesn't have a `new_debug` field yet).

- [ ] **Step 3: Update `ChangeKind` and `diff_classify`**

Replace the `ChangeKind` enum and `diff_classify` body in `crates/bot/src/config_watcher.rs`:

```rust
/// Classification of a single agent.yaml change event.
#[derive(Debug)]
pub(crate) enum ChangeKind {
    /// File contents bytewise unchanged — fs noise (mtime touch, atomic
    /// rename, etc.). Skip silently.
    NoChange,
    /// Only `model` and/or `debug` changed — apply in-memory and continue running.
    HotReloadable {
        new_model: Option<String>,
        new_debug: Option<bool>,
    },
    /// Anything else — graceful restart.
    RestartRequired,
}

/// Decide whether a change can be hot-reloaded or requires a restart.
///
/// Compares old + new yaml as parsed `AgentConfig` values with `model` and
/// `debug` nulled out on both sides. If the rest is equal, hot-reload;
/// else restart. Parse failure on either side fails-safe to restart.
pub(crate) fn diff_classify(old_yaml: &str, new_yaml: &str) -> ChangeKind {
    if old_yaml == new_yaml {
        return ChangeKind::NoChange;
    }
    let mut old: AgentConfig = match serde_saphyr::from_str(old_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %format!("{e:#}"),
                "config_watcher: failed to parse old agent.yaml — restart required"
            );
            return ChangeKind::RestartRequired;
        }
    };
    let mut new: AgentConfig = match serde_saphyr::from_str(new_yaml) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %format!("{e:#}"),
                "config_watcher: failed to parse new agent.yaml — restart required"
            );
            return ChangeKind::RestartRequired;
        }
    };
    let new_model = new.model.take();
    let new_debug = new.debug.take();
    old.model = None;
    old.debug = None;
    if old == new {
        ChangeKind::HotReloadable { new_model, new_debug }
    } else {
        ChangeKind::RestartRequired
    }
}
```

- [ ] **Step 4: Run tests — both new and pre-existing should pass**

```bash
cargo test -p right-bot diff_ -- --nocapture
```

Expected: all tests in the `config_watcher::tests` module pass, including the 5 new ones AND the pre-existing model-only tests (which still match the new `HotReloadable { new_model, new_debug }` pattern — `new_debug` is `None` for them).

You will see compile errors in `spawn_config_watcher` because the destructure pattern in the watcher loop is now stale. That is fixed in Step 5.

- [ ] **Step 5: Update `spawn_config_watcher` to receive and apply the debug flag**

In `crates/bot/src/config_watcher.rs`, update the function signature and the inner watcher thread:

```rust
pub(crate) fn spawn_config_watcher(
    agent_yaml: &Path,
    token: CancellationToken,
    config_changed: Arc<AtomicBool>,
    model_swap: Arc<ArcSwap<Option<String>>>,
    debug_flag: Arc<AtomicBool>,
) -> miette::Result<()> {
```

Then in the match arm inside the watcher thread, replace the existing
`HotReloadable { new_model } =>` block with:

```rust
                        ChangeKind::HotReloadable { new_model, new_debug } => {
                            tracing::info!(
                                model = ?new_model.as_deref().unwrap_or("default"),
                                debug = ?new_debug,
                                "agent.yaml: model/debug-only change — hot-reloading"
                            );
                            model_swap.store(Arc::new(new_model));
                            // None means "field absent" — preserve current AtomicBool value.
                            // Some(v) means "set to v".
                            if let Some(v) = new_debug {
                                debug_flag.store(v, Ordering::Release);
                            }
                            last_yaml = new_yaml;
                        }
```

- [ ] **Step 6: Build to find call sites that need updating**

```bash
cargo build -p right-bot 2>&1 | head -30
```

Expected: one error pointing at the `spawn_config_watcher(...)` call in
`crates/bot/src/lib.rs` (around line 466) — missing the new `debug_flag`
argument. We fix that in Task 6.

- [ ] **Step 7: Run config_watcher tests in isolation to confirm they still pass**

```bash
cargo test -p right-bot --lib config_watcher 2>&1 | tail -20
```

Expected: tests compile and pass, even though the rest of `right-bot`
won't link yet (callers of `spawn_config_watcher` are stale). If
`cargo test --lib` fails to link entirely, this step is observational
only — the next task fixes the call sites and we'll re-test the whole
crate then.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/config_watcher.rs
git commit -m "feat(bot): config_watcher hot-reloads debug flag alongside model"
```

---

## Task 5: Add `snapshot_debug` helper in `bot/src/lib.rs`

Mirrors `snapshot_model`. Used by ClaudeInvocation builders to read the
current debug state.

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Add the helper next to `snapshot_model`**

In `crates/bot/src/lib.rs`, find `pub(crate) fn snapshot_model` (around
line 22). Add immediately after it:

```rust
/// Read the current debug-flag value with relaxed ordering. Same purpose as
/// `snapshot_model` for the debug AtomicBool.
pub(crate) fn snapshot_debug(cell: &std::sync::atomic::AtomicBool) -> bool {
    cell.load(std::sync::atomic::Ordering::Relaxed)
}
```

- [ ] **Step 2: Verify it compiles in isolation**

```bash
cargo check -p right-bot --lib 2>&1 | head -10
```

Expected: still complains about `spawn_config_watcher` arity (Task 4), but
the new helper itself compiles. The full fix is Task 6.

- [ ] **Step 3: No commit yet — bundle with Task 6 since they form one wiring change.**

---

## Task 6: Wire `debug_flag: Arc<AtomicBool>` through `bot::run`, `AgentSettings`, watcher, telegram

The bot constructs the `Arc<AtomicBool>` at startup, passes it to the watcher (writer) and stores it in `AgentSettings` (readers).

**Files:**
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/handler.rs`

- [ ] **Step 1: Update `AgentSettings.debug` to `Arc<AtomicBool>`**

In `crates/bot/src/telegram/handler.rs` around line 102, replace:

```rust
    /// When true, CC subprocesses run with --verbose and stderr is logged at debug level.
    pub debug: bool,
```

With:

```rust
    /// Hot-reloadable debug flag. When true, CC subprocesses run with --verbose,
    /// stderr is logged at debug level, AND `claude` runs with --debug --debug-file=...
    /// Updated by `/debug` Telegram command and config_watcher (yaml diff). Read on
    /// every CC invocation.
    pub debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
```

- [ ] **Step 2: Construct the AtomicBool in `bot::run` and plumb through**

In `crates/bot/src/lib.rs`, find the block creating `model_arc` (around line 464). Replace the model_arc + spawn_config_watcher block with:

```rust
    // Hot-reloadable debug flag. yaml takes precedence; CLI --debug is the fallback.
    let initial_debug = config.debug.unwrap_or(args.debug);
    let debug_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(initial_debug));

    // Create the model swap cell here so both the watcher and the telegram
    // dispatcher share the same Arc. The watcher writes; the dispatcher reads.
    let model_arc: Arc<arc_swap::ArcSwap<Option<String>>> =
        Arc::new(arc_swap::ArcSwap::from_pointee(config.model.clone()));
    config_watcher::spawn_config_watcher(
        &agent_yaml_path,
        shutdown.clone(),
        Arc::clone(&config_changed),
        Arc::clone(&model_arc),
        Arc::clone(&debug_flag),
    )?;
```

- [ ] **Step 3: Pass `debug_flag` to `run_telegram` instead of the bool**

In `crates/bot/src/lib.rs`, find the `telegram::run_telegram(...)` call (around line 928). The current call passes `args.debug` as a positional `bool` (line 932). Replace with `Arc::clone(&debug_flag)`:

```rust
        result = telegram::run_telegram(
            token,
            allowlist,
            agent_dir,
            Arc::clone(&debug_flag),
            // ... rest unchanged
```

- [ ] **Step 4: Update `run_telegram` signature**

Find `pub async fn run_telegram(` in `crates/bot/src/telegram/dispatch.rs`. Change the `debug: bool` parameter to:

```rust
    debug: std::sync::Arc<std::sync::atomic::AtomicBool>,
```

- [ ] **Step 5: Update `AgentSettings { ... }` constructions**

Find every place `AgentSettings { ... debug: ..., ... }` is built. There
are at least two — `crates/bot/src/telegram/handler.rs:362` and
`crates/bot/src/telegram/dispatch.rs:549`. Confirm:

```bash
rg -n "debug: " crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs | head -10
```

Replace `debug: <whatever bool>` with `debug: <Arc<AtomicBool>>`.

In `dispatch.rs:549` (test fixture `for_test`), replace `debug: false`
with:

```rust
        debug: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
```

In `handler.rs:362` (the construction inside the worker spawn), pass
`Arc::clone(&settings.debug)` if you can — otherwise, find where the
`debug` value originates in that scope and replace appropriately. Use
`rg -n "debug" crates/bot/src/telegram/handler.rs` to locate the
context.

- [ ] **Step 6: Update read sites that previously did `settings.debug` (bool)**

`rg -n "settings.debug|args.debug|\.debug" crates/bot/src/ -g '!*tests*' -g '!*test*' | head`

For each read of the old bool field, replace with
`crate::snapshot_debug(&settings.debug)`. Worker (`crates/bot/src/telegram/worker.rs`)
will be one of them.

- [ ] **Step 7: Build the whole crate**

```bash
cargo build -p right-bot 2>&1 | tail -20
```

Expected: builds. If errors remain, they will be type-mismatch sites
the grep didn't catch — fix one at a time using `crate::snapshot_debug`.

- [ ] **Step 8: Run all bot tests**

```bash
cargo test -p right-bot --lib 2>&1 | tail -20
```

Expected: pass. Pre-existing tests may need their fixtures updated to
construct `Arc<AtomicBool>` instead of `false`. Fix them.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/lib.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs
# plus any other files touched in Step 6
git commit -m "feat(bot): hot-reloadable debug flag plumbed through AgentSettings"
```

---

## Task 7: ClaudeInvocation emits `--debug --debug-file=...` when flag is on

The flag is read at `into_args` time, not at construction.

**Files:**
- Modify: `crates/bot/src/cc/invocation.rs`

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/bot/src/cc/invocation.rs`:

```rust
    #[test]
    fn debug_flag_true_appends_debug_and_debug_file() {
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(args.contains(&"--debug".to_string()), "expected --debug:\n{args:?}");
        assert!(
            args.iter().any(|a| a == "--debug-file=/sandbox/.claude/logs/abc-123.log"),
            "expected --debug-file=/sandbox/.claude/logs/abc-123.log:\n{args:?}"
        );
    }

    #[test]
    fn debug_flag_false_omits_debug_and_debug_file() {
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(!args.contains(&"--debug".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("--debug-file=")));
    }

    #[test]
    fn debug_flag_absent_omits_debug() {
        // No debug_flag set at all (None) — should behave like false.
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let args = inv.into_args();
        assert!(!args.contains(&"--debug".to_string()));
    }

    #[test]
    fn debug_flag_uses_resume_session_id_when_no_fork() {
        let mut inv = minimal();
        inv.resume_session_id = Some("resume-uuid".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(
            args.iter().any(|a| a == "--debug-file=/sandbox/.claude/logs/resume-uuid.log"),
            "with --resume (no fork), debug-file should use resume-uuid:\n{args:?}"
        );
    }

    #[test]
    fn debug_flag_uses_new_session_id_when_forking() {
        let mut inv = minimal();
        inv.resume_session_id = Some("old-uuid".into());
        inv.new_session_id = Some("new-uuid".into());
        inv.fork_session = true;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(
            args.iter().any(|a| a == "--debug-file=/sandbox/.claude/logs/new-uuid.log"),
            "with --fork-session, debug-file should use new session id (CC writes JSONL by new id):\n{args:?}"
        );
    }

    #[test]
    fn debug_flag_runtime_toggle_picked_up_at_build_time() {
        let mut inv = minimal();
        inv.new_session_id = Some("abc-123".into());
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        // Flip after construction.
        flag.store(true, std::sync::atomic::Ordering::Release);
        let args = inv.into_args();
        assert!(args.contains(&"--debug".to_string()), "build-time read must observe the flip");
    }

    #[test]
    fn debug_flag_no_session_id_omits_debug_file_but_still_emits_debug() {
        let mut inv = minimal();
        // Neither resume nor new session id set.
        inv.resume_session_id = None;
        inv.new_session_id = None;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        inv.debug_flag = Some(std::sync::Arc::clone(&flag));
        let args = inv.into_args();
        assert!(args.contains(&"--debug".to_string()));
        // No --debug-file because we have no session UUID to put in the path.
        assert!(!args.iter().any(|a| a.starts_with("--debug-file=")));
    }
```

You also need the `minimal()` helper to compile; it currently does NOT
have a `debug_flag` field. After Step 3 below it will, so these tests
will compile with the addition. Until then they will fail to compile —
that is the "failing test" state (compile-time failure is acceptable
under TDD).

- [ ] **Step 2: Run tests to verify they fail (won't compile)**

```bash
cargo test -p right-bot --lib invocation::tests::debug_flag 2>&1 | tail -20
```

Expected: compile error, "no field `debug_flag` on `ClaudeInvocation`".

- [ ] **Step 3: Add `debug_flag` field to `ClaudeInvocation` and an effective-session-id helper**

In `crates/bot/src/cc/invocation.rs`, add the field at the bottom of
the struct:

```rust
pub(crate) struct ClaudeInvocation {
    // ... existing fields ...
    pub(crate) prompt: Option<String>,
    /// Hot-reloadable debug toggle. None = off (treated as false).
    /// When true at `into_args()` time, appends `--debug --debug-file=<path>`.
    pub(crate) debug_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
```

Update `minimal()` in the test module to set `debug_flag: None,` so the
existing tests still build.

Add the helper method (place it inside `impl ClaudeInvocation`, just
above `into_args`):

```rust
    /// Returns the session UUID CC will write its JSONL under, matching
    /// the args `into_args` will emit. Used to compute --debug-file path.
    ///
    /// - `--fork-session` + `new_session_id` → CC creates a new file by `new_session_id`.
    /// - `--resume <id>` (no fork) → CC continues writing to `<id>.jsonl`.
    /// - `--session-id <id>` (no resume) → CC writes to `<id>.jsonl`.
    /// - Neither set → CC generates its own UUID; we cannot know in advance.
    pub(crate) fn effective_session_id(&self) -> Option<&str> {
        if self.fork_session && let Some(id) = &self.new_session_id {
            return Some(id.as_str());
        }
        if let Some(id) = &self.resume_session_id {
            return Some(id.as_str());
        }
        self.new_session_id.as_deref()
    }
```

Then in `into_args(self)`, after step 8 (Extra args, around line 129)
and BEFORE step 9 (Output format) — debug flags must come before
`--verbose` to keep prompt-cache-friendly arg ordering — add:

```rust
        // 8.5: Debug flag (hot-reloadable, read at build time).
        let debug_on = self
            .debug_flag
            .as_ref()
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed));
        if debug_on {
            args.push("--debug".into());
            // effective_session_id consumes &self; clone the relevant field first
            // since `self.fork_session` etc. are by value here.
            let sid_for_path: Option<String> = if self.fork_session && self.new_session_id.is_some() {
                self.new_session_id.clone()
            } else if let Some(id) = &self.resume_session_id {
                Some(id.clone())
            } else {
                self.new_session_id.clone()
            };
            if let Some(sid) = sid_for_path {
                args.push(format!("--debug-file=/sandbox/.claude/logs/{sid}.log"));
            }
        }
```

(Using a local closure-style block instead of `effective_session_id()`
because `into_args` consumes `self` and `effective_session_id` borrows.
Keep `effective_session_id` available for non-consuming callers.)

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p right-bot --lib invocation::tests 2>&1 | tail -20
```

Expected: all 7 new tests pass; pre-existing tests still pass.

- [ ] **Step 5: Update every ClaudeInvocation construction site to pass the debug flag**

Find every site:

```bash
rg -n "ClaudeInvocation \{" crates/bot/src/ | head -10
```

For each site (`worker.rs`, `cron.rs`, `cron_delivery.rs`, `reflection.rs`):

Add `debug_flag: Some(Arc::clone(&ctx.debug))` (or wherever the
AgentSettings/context AtomicBool is reachable). For sites without a
context, add it explicitly — pass through from the caller.

- [ ] **Step 6: Build to verify everything compiles**

```bash
cargo build -p right-bot 2>&1 | tail -20
```

Expected: clean build.

- [ ] **Step 7: Run all bot tests**

```bash
cargo test -p right-bot 2>&1 | tail -20
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/bot/src/cc/invocation.rs
# plus every callsite touched in Step 5
git commit -m "feat(bot): ClaudeInvocation emits --debug --debug-file when flag is on"
```

---

## Task 8: Register `/rightreflect` skill in the codegen installer

**Files:**
- Modify: `crates/right-codegen/src/skills.rs`

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/right-codegen/src/skills.rs`:

```rust
    #[test]
    fn installs_rightreflect_skill() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        assert!(
            dir.path()
                .join(".claude/skills/rightreflect/SKILL.md")
                .exists(),
            "rightreflect/SKILL.md should exist"
        );
    }

    #[test]
    fn rightreflect_skill_frontmatter_is_valid() {
        let dir = tempdir().unwrap();
        install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
        let content = std::fs::read_to_string(
            dir.path().join(".claude/skills/rightreflect/SKILL.md"),
        )
        .unwrap();
        assert!(content.starts_with("---\n"), "frontmatter must start file");
        assert!(content.contains("name: rightreflect"), "must declare name");
        assert!(
            content.contains("/sandbox/.claude/projects/-sandbox/"),
            "must reference the JSONL path"
        );
    }
```

Update the `all_source_skill_files_are_installed` test (around line 266) so the `skills` array also lists rightreflect:

```rust
        let skills: &[(&str, &str)] = &[
            ("rightskills", "rightskills"),
            ("rightcron", "rightcron"),
            ("rightmcp", "rightmcp"),
            ("rightmemory-file", "rightmemory"),
            ("rightreflect", "rightreflect"),
        ];
```

- [ ] **Step 2: Run the new tests to verify they fail**

```bash
cargo test -p right-codegen rightreflect -- --nocapture
```

Expected: failures — `include_dir!` panic at compile time, OR the
target file does not exist after install.

- [ ] **Step 3: Register the skill**

In `crates/right-codegen/src/skills.rs`, near the top of the file (after the existing `const SKILL_*: Dir` lines):

```rust
const SKILL_RIGHTREFLECT: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/rightreflect");
```

In `install_builtin_skills`, append to the `skills` array:

```rust
    let skills: &[(&str, &Dir)] = &[
        ("rightskills", &SKILL_RIGHTSKILLS),
        ("rightcron", &SKILL_RIGHTCRON),
        ("rightmcp", &SKILL_RIGHTMCP),
        ("rightmemory", rightmemory_dir),
        ("rightreflect", &SKILL_RIGHTREFLECT),
    ];
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p right-codegen rightreflect -- --nocapture
cargo test -p right-codegen all_source_skill_files_are_installed -- --nocapture
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/src/skills.rs
git commit -m "feat(codegen): bundle and install /rightreflect skill"
```

---

## Task 9: Implement `/debug` Telegram command handler

**Files:**
- Create: `crates/bot/src/telegram/debug_command.rs`
- Modify: `crates/bot/src/telegram/mod.rs`

- [ ] **Step 1: Register the new module**

In `crates/bot/src/telegram/mod.rs`, find `pub(crate) mod model_command;` (line 11) and add right below:

```rust
pub(crate) mod debug_command;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/bot/src/telegram/debug_command.rs` with skeleton + tests:

```rust
//! `/debug` command — toggle hot-reloadable debug flag.
//!
//! UI: text-only command (`/debug`, `/debug on`, `/debug off`). No inline
//! keyboard — the option set is binary, no point in a 2-button menu.
//!
//! Persistence: writes `agent.yaml::debug` via
//! `right_agent::agent::types::write_agent_yaml_debug`.
//! In-memory: stores into `AgentSettings.debug: Arc<AtomicBool>`.
//! Group chats are gated by the trusted-users allowlist.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DebugAction {
    Status,
    On,
    Off,
}

/// Parse the optional argument after `/debug`. Trims whitespace and
/// is case-insensitive. Empty / missing → Status.
pub(crate) fn parse_debug_action(args: &str) -> Result<DebugAction, String> {
    let s = args.trim().to_ascii_lowercase();
    match s.as_str() {
        "" => Ok(DebugAction::Status),
        "on" | "true" | "1" => Ok(DebugAction::On),
        "off" | "false" | "0" => Ok(DebugAction::Off),
        other => Err(format!("Unknown argument: {other}. Use `/debug on`, `/debug off`, or `/debug` (status).")),
    }
}

/// Format the status reply when no action was given. Includes a hint about
/// the per-session debug log file when present.
pub(crate) fn render_status(debug_on: bool, current_log_size: Option<u64>) -> String {
    if debug_on {
        let log_part = match current_log_size {
            Some(size) => format!("\n\nCurrent session log: {size} bytes."),
            None => "\n\nNo log written yet for the current session.".to_string(),
        };
        format!(
            "🐛 Debug mode is ON.\n\n\
             Future `claude -p` invocations will write API/transport logs to \
             `/sandbox/.claude/logs/<session>.log`. Use `/debug off` to disable.\
             {log_part}"
        )
    } else {
        "🐛 Debug mode is OFF.\n\n\
         Use `/debug on` to enable per-session API/transport logs at \
         `/sandbox/.claude/logs/<session>.log`. Existing CC project history at \
         `/sandbox/.claude/projects/-sandbox/*.jsonl` is always written; debug \
         mode adds deeper API-layer detail.".to_string()
    }
}

/// Format the reply after a successful toggle.
pub(crate) fn render_toggle(new_value: bool) -> String {
    if new_value {
        "🐛 Debug mode ON. Future turns will write API/transport logs to \
         `/sandbox/.claude/logs/<session>.log`. Past turns are unchanged.".to_string()
    } else {
        "🐛 Debug mode OFF. Existing logs remain.".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_no_arg_is_status() {
        assert_eq!(parse_debug_action("").unwrap(), DebugAction::Status);
        assert_eq!(parse_debug_action("   ").unwrap(), DebugAction::Status);
    }

    #[test]
    fn parse_on_synonyms() {
        assert_eq!(parse_debug_action("on").unwrap(), DebugAction::On);
        assert_eq!(parse_debug_action("ON").unwrap(), DebugAction::On);
        assert_eq!(parse_debug_action(" on ").unwrap(), DebugAction::On);
        assert_eq!(parse_debug_action("true").unwrap(), DebugAction::On);
        assert_eq!(parse_debug_action("1").unwrap(), DebugAction::On);
    }

    #[test]
    fn parse_off_synonyms() {
        assert_eq!(parse_debug_action("off").unwrap(), DebugAction::Off);
        assert_eq!(parse_debug_action("OFF").unwrap(), DebugAction::Off);
        assert_eq!(parse_debug_action("false").unwrap(), DebugAction::Off);
        assert_eq!(parse_debug_action("0").unwrap(), DebugAction::Off);
    }

    #[test]
    fn parse_unknown_is_error() {
        let err = parse_debug_action("toggle").unwrap_err();
        assert!(err.contains("Unknown argument"));
        assert!(err.contains("toggle"));
    }

    #[test]
    fn status_off_explains_what_on_would_do() {
        let s = render_status(false, None);
        assert!(s.contains("OFF"));
        assert!(s.contains("/debug on"));
        assert!(s.contains("/sandbox/.claude/logs/"));
    }

    #[test]
    fn status_on_with_log_size_reports_bytes() {
        let s = render_status(true, Some(2048));
        assert!(s.contains("ON"));
        assert!(s.contains("2048 bytes"));
    }

    #[test]
    fn status_on_without_log_says_so() {
        let s = render_status(true, None);
        assert!(s.contains("ON"));
        assert!(s.contains("No log written"));
    }

    #[test]
    fn toggle_on_message_mentions_future_turns() {
        let s = render_toggle(true);
        assert!(s.contains("ON"));
        assert!(s.contains("Future turns"));
    }

    #[test]
    fn toggle_off_message_mentions_existing_logs() {
        let s = render_toggle(false);
        assert!(s.contains("OFF"));
        assert!(s.contains("Existing logs remain"));
    }
}

/// Apply a `DebugAction`: persist to yaml, flip the AtomicBool. Returns
/// the message to show the user. Persists BEFORE swapping in-memory so that
/// a disk failure leaves runtime untouched.
pub(crate) fn apply_action(
    action: DebugAction,
    flag: &Arc<AtomicBool>,
    agent_yaml_path: &std::path::Path,
    current_log_size: Option<u64>,
) -> Result<String, String> {
    match action {
        DebugAction::Status => Ok(render_status(flag.load(Ordering::Relaxed), current_log_size)),
        DebugAction::On | DebugAction::Off => {
            let new_value = action == DebugAction::On;
            right_agent::agent::types::write_agent_yaml_debug(agent_yaml_path, Some(new_value))
                .map_err(|e| format!("Failed to save debug flag: {e:#}"))?;
            flag.store(new_value, Ordering::Release);
            Ok(render_toggle(new_value))
        }
    }
}

/// teloxide handler — registered in dispatch.rs. The `args` String comes from
/// `BotCommand::Debug(args)` (whitespace-trimmed by teloxide).
pub(crate) async fn handle_debug(
    bot: super::BotType,
    msg: teloxide::types::Message,
    args: String,
    settings: std::sync::Arc<super::handler::AgentSettings>,
    agent_dir: std::sync::Arc<super::handler::AgentDir>,
    allowlist: right_agent::agent::allowlist::AllowlistHandle,
) -> teloxide::prelude::ResponseResult<()> {
    use teloxide::prelude::*;

    if !super::handler::is_private_chat(&msg.chat.kind)
        && !super::allowlist_commands::sender_is_trusted(&msg, &allowlist)
    {
        tracing::debug!(
            chat_id = msg.chat.id.0,
            user_id = msg.from.as_ref().map(|u| u.id.0),
            "/debug ignored: non-trusted sender in group"
        );
        return Ok(());
    }

    let action = match parse_debug_action(&args) {
        Ok(a) => a,
        Err(e) => {
            send_reply(&bot, &msg, &e).await?;
            return Ok(());
        }
    };

    // For the status response we want the size of the current session's log.
    // Worker tracks the active CC session_id per chat; reading it here would
    // require plumbing. As a simpler proxy, read the most-recent file size
    // in /sandbox/.claude/logs/ (sandbox-side via SSH would be needed —
    // not worth the complexity for the status hint). Pass None.
    let current_log_size: Option<u64> = None;

    let agent_yaml_path = agent_dir.0.join("agent.yaml");
    let reply = match apply_action(action, &settings.debug, &agent_yaml_path, current_log_size) {
        Ok(s) => s,
        Err(e) => e,
    };

    send_reply(&bot, &msg, &reply).await?;
    Ok(())
}

async fn send_reply(
    bot: &super::BotType,
    msg: &teloxide::types::Message,
    text: &str,
) -> teloxide::prelude::ResponseResult<()> {
    use teloxide::prelude::*;
    let mut send = bot
        .send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::Html);
    if let Some(thread_id) = msg.thread_id {
        send = send.message_thread_id(thread_id);
    }
    send.await?;
    Ok(())
}
```

- [ ] **Step 3: Run tests to verify they pass**

```bash
cargo test -p right-bot debug_command -- --nocapture
```

Expected: all 9 unit tests pass. The handler itself is not unit-tested
in isolation (relies on teloxide types) but is exercised via integration
manual checklist.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/telegram/debug_command.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): /debug Telegram command handler with on/off/status"
```

---

## Task 10: Wire `/debug` into `BotCommand` and dispatch

**Files:**
- Modify: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Add the variant to `BotCommand`**

In `crates/bot/src/telegram/dispatch.rs`, find the `enum BotCommand` (around line 39). Add:

```rust
    /// Toggle hot-reloadable debug mode. Use `/debug` for status, `/debug on`,
    /// `/debug off`. When on, claude -p runs with --debug --debug-file=...
    Debug(String),
```

(Place after `Model` to keep grouping consistent with `model_command.rs`'s sibling position.)

- [ ] **Step 2: Register the dispatch branch**

Find the `dptree::case![BotCommand::Model].endpoint(handle_model)` line (around line 404) and add right below:

```rust
        .branch(dptree::case![BotCommand::Debug(args)].endpoint(super::debug_command::handle_debug))
```

- [ ] **Step 3: Confirm BotCommands metadata refresh works**

```bash
cargo build -p right-bot 2>&1 | tail -10
```

Expected: clean. The `BotCommand::bot_commands()` invocation that publishes the command list to Telegram (around line 293) automatically picks up new variants — no extra wiring.

- [ ] **Step 4: Run integration-level dispatch tests**

```bash
cargo test -p right-bot --lib telegram 2>&1 | tail -20
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/dispatch.rs
git commit -m "feat(bot): register /debug command in dispatch"
```

---

## Task 11: Update `OPERATING_INSTRUCTIONS.md` with skill + command pointers

**Files:**
- Modify: `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`

- [ ] **Step 1: Add `/rightreflect` to Core Skills**

Open `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`. Find the `## Core Skills` section (line 201). Replace the placeholder comment with a real entry:

```markdown
## Core Skills

- `/rightreflect` — read your own past sessions when the user asks "why did you ...?". Reads CC's project JSONL inside the sandbox. No MCP calls, no DB.

<!-- Add additional skills here. Example: -->
<!-- - `/my-skill` -- description of what it does -->
```

- [ ] **Step 2: Add `/debug` to a user-controls hint**

Find the `## MCP Management` section (line 57) which lists Telegram commands the user controls. Add a parallel mention near the bottom of that section or after it. Concretely, after the closing of the `## MCP Management` block (before `## Communication` at line 74), insert:

```markdown
## Debug Mode

The user can toggle deeper API/transport logging by sending `/debug on` or
`/debug off` in this chat. When on, `claude -p` runs with `--debug
--debug-file=/sandbox/.claude/logs/<session>.log`. The `/rightreflect` skill
reads these logs as a fallback when the JSONL alone doesn't explain a past
behavior. You cannot toggle debug mode yourself — only the user can.
```

- [ ] **Step 3: Verify the file still passes minijinja templating**

The skill installer renders `OPERATING_INSTRUCTIONS.md` is NOT in the path of the minijinja renderer (only files inside `skills/` are). No template syntax to worry about.

- [ ] **Step 4: Commit**

```bash
git add crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "docs(prompt): introduce /rightreflect skill and /debug command to agents"
```

---

## Task 12: Add discoverability to `agent.yaml` template

**Files:**
- Modify: `crates/right-agent/templates/right/agent/agent.yaml`

- [ ] **Step 1: Read current contents**

```bash
cat crates/right-agent/templates/right/agent/agent.yaml
```

Identify a sensible location (after `model:` if present, otherwise near other operational toggles like `show_thinking:`).

- [ ] **Step 2: Add commented `# debug:` discoverability line**

Append (or insert in topical order) a block like:

```yaml

# Debug mode: when true, `claude -p` runs with --debug --debug-file=...
# Toggle live in Telegram via `/debug on` / `/debug off`. Persists here.
# debug: false
```

The line stays commented out by default — `Option<bool>::None` keeps
behavior identical to today.

- [ ] **Step 3: Verify the file still parses if uncommented**

```bash
cargo test -p right-core agent_config_debug -- --nocapture
```

Expected: pre-existing parse tests for the new field still pass (they
parse YAML strings, not the template — but if the template were
malformed the agent-init flow would later choke; we sanity-check now).

- [ ] **Step 4: Commit**

```bash
git add crates/right-agent/templates/right/agent/agent.yaml
git commit -m "docs(template): add commented debug field to agent.yaml template"
```

---

## Task 13: Update top-level architecture docs

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `PROMPT_SYSTEM.md`
- Modify: `docs/architecture/sessions.md`

- [ ] **Step 1: Update `ARCHITECTURE.md` — Hot-reloadable fields list**

Find the paragraph "**Hot-reloadable fields in `agent.yaml`.**" (search
for it). Replace the body with:

> Most fields trigger a graceful restart on change (via `config_watcher`).
> Two exceptions: `model` and `debug`. The watcher's smart-diff classifies
> a `model`/`debug`-only change as hot-reloadable and stores the new
> values into `AgentSettings.model` (an `Arc<ArcSwap<...>>`) and
> `AgentSettings.debug` (an `Arc<AtomicBool>`) without restarting. The
> Telegram `/model` and `/debug` commands exploit this path — in-flight
> CC subprocesses keep their old flags; the next invocation in any chat
> picks up the new value. Adding more hot-reloadable fields requires
> extending the diff in `crates/bot/src/config_watcher.rs::diff_classify`.

- [ ] **Step 2: Update `ARCHITECTURE.md` — Directory Layout (Runtime)**

Find the `## Directory Layout (Runtime)` section. Add `/sandbox/.claude/logs/`
to the per-agent paths description, e.g. at the end of the
`agents/<name>/` bullet:

> Sandbox-internal: `/sandbox/.claude/projects/-sandbox/<sid>.jsonl`
> (CC project history, agent-readable for self-introspection via the
> `/rightreflect` skill); `/sandbox/.claude/logs/<sid>.log` (CC debug
> output, only present when `/debug` is on).

- [ ] **Step 3: Update `PROMPT_SYSTEM.md` — note the conditional debug args**

Find a section discussing CC invocation flags (likely "Claude Invocation
Contract" or similar). Add:

> When `agent.yaml::debug` (hot-reloadable via `/debug` Telegram command)
> is true, ClaudeInvocation also appends `--debug
> --debug-file=/sandbox/.claude/logs/<session-uuid>.log`. The session
> UUID matches CC's own JSONL filename. Off by default.

- [ ] **Step 4: Update `docs/architecture/sessions.md`**

Add a section titled "Self-introspection" near the end:

```markdown
## Self-introspection

Every CC invocation writes its full conversation graph to
`/sandbox/.claude/projects/-sandbox/<session-uuid>.jsonl` inside the
sandbox. The session UUID matches the `--session-id` we pass to
`claude`, so the bot's session UUIDs (from the `sessions` table) and
`cron_runs.id` map directly to JSONL filenames.

The `/rightreflect` bundled skill teaches the agent to read these
files when the user asks "why did you ...?". When `/debug` is on,
ClaudeInvocation also writes per-session API-layer detail to
`/sandbox/.claude/logs/<session-uuid>.log` — same UUID, parallel
sandbox path. The skill consults that file as a fallback when the
JSONL alone doesn't explain a past behavior.
```

- [ ] **Step 5: Commit**

```bash
git add ARCHITECTURE.md PROMPT_SYSTEM.md docs/architecture/sessions.md
git commit -m "docs(architecture): document hot-reloadable debug + self-introspection paths"
```

---

## Task 14: Integration test — `cc_debug_file_lands_inside_sandbox`

**Files:**
- Modify: `crates/bot/src/cc/invocation.rs` (test only) OR
- Create: `crates/bot/tests/cc_debug_integration.rs`

We add the test at the integration-test level (`tests/`) so the
`test-support` feature wiring works without polluting the unit-test
module.

- [ ] **Step 1: Confirm `right-core` test-support feature is enabled**

```bash
rg -n 'right-core.*test-support|features = \["test-support"\]' crates/bot/Cargo.toml
```

Expected: present. If not, add `right-core = { ..., features = ["test-support"] }` under `[dev-dependencies]`.

- [ ] **Step 2: Write the integration test**

Create `crates/bot/tests/cc_debug_integration.rs`:

```rust
//! Integration: confirm `claude -p --debug --debug-file=/sandbox/.claude/logs/<sid>.log`
//! actually lands a file inside a live OpenShell sandbox.
//!
//! Validates the load-bearing assumption of the /rightreflect skill: that
//! enabling debug mode produces an agent-readable per-session log.

use right_core::test_support::TestSandbox;

#[tokio::test]
async fn cc_debug_file_lands_inside_sandbox() {
    let sandbox = TestSandbox::create("rightreflect-debugfile")
        .await
        .expect("create sandbox");
    let session_id = "rightreflect-test-00000000-0000-0000-0000-000000000001";
    let log_path = format!("/sandbox/.claude/logs/{session_id}.log");

    // Pre-create the logs directory; CC will write to it.
    sandbox
        .exec(&["mkdir", "-p", "/sandbox/.claude/logs"])
        .await
        .expect("mkdir logs");

    // Minimal claude -p call. We don't need to authenticate or get a real
    // response — we only need claude to write its --debug-file. Use a tiny
    // schema-constrained call that fails fast (no auth token in sandbox)
    // but still emits debug output before failing.
    let _ignored = sandbox
        .exec(&[
            "claude",
            "-p",
            "--dangerously-skip-permissions",
            "--debug",
            &format!("--debug-file={log_path}"),
            "--session-id",
            session_id,
            "--",
            "hello",
        ])
        .await;
    // We do NOT assert success — claude will fail without a real auth token,
    // but it should still create the debug file before bailing.

    let ls = sandbox
        .exec(&["ls", "-la", &log_path])
        .await
        .expect("ls debug file");
    assert!(
        ls.stdout.contains(session_id),
        "debug file not found at {log_path}:\nstdout={}\nstderr={}",
        ls.stdout,
        ls.stderr
    );

    let stat = sandbox
        .exec(&["wc", "-c", &log_path])
        .await
        .expect("wc debug file");
    let bytes: u64 = stat
        .stdout
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .expect("parse byte count");
    assert!(bytes > 0, "debug file is empty: {stat:?}");
}
```

- [ ] **Step 3: Run the test**

```bash
cargo test -p right-bot --test cc_debug_integration -- --nocapture
```

Expected: test passes against the live OpenShell daemon. If it fails because `claude` CLI is missing inside the sandbox, the assumption breaks — debug this against the actual sandbox state via `ssh -F ~/.right/run/ssh/...`.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/tests/cc_debug_integration.rs
# Cargo.toml if you had to add the feature
git commit -m "test(bot): integration test that --debug-file lands in sandbox"
```

---

## Task 15: Integration test — `jsonl_file_exists_for_invoked_session`

Sanity-check the assumption that holds the whole skill up: CC writes
`/sandbox/.claude/projects/-sandbox/<sid>.jsonl` automatically.

**Files:**
- Modify: `crates/bot/tests/cc_debug_integration.rs` (add a second test)

- [ ] **Step 1: Append the second test**

In `crates/bot/tests/cc_debug_integration.rs`:

```rust
#[tokio::test]
async fn jsonl_file_exists_for_invoked_session() {
    let sandbox = TestSandbox::create("rightreflect-jsonl")
        .await
        .expect("create sandbox");
    let session_id = "rightreflect-test-00000000-0000-0000-0000-000000000002";
    let jsonl_path = format!("/sandbox/.claude/projects/-sandbox/{session_id}.jsonl");

    let _ignored = sandbox
        .exec(&[
            "claude",
            "-p",
            "--dangerously-skip-permissions",
            "--session-id",
            session_id,
            "--",
            "hello",
        ])
        .await;

    let ls = sandbox
        .exec(&["ls", "-la", &jsonl_path])
        .await
        .expect("ls jsonl file");
    assert!(
        ls.stdout.contains(session_id),
        "jsonl file not found at {jsonl_path}:\nstdout={}\nstderr={}",
        ls.stdout,
        ls.stderr
    );
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p right-bot --test cc_debug_integration jsonl_file -- --nocapture
```

Expected: passes. If CC fails before writing the JSONL (auth error very early) we may need to use `--mcp-config` and `--strict-mcp-config` with a stub config to push CC further along. Adjust if so.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/tests/cc_debug_integration.rs
git commit -m "test(bot): verify CC always writes session JSONL in sandbox"
```

---

## Task 16: Integration test — `skill_can_grep_jsonl`

Confirms the skill's primary mechanic (`grep -l` for keyword in JSONL files) works inside the sandbox env.

**Files:**
- Modify: `crates/bot/tests/cc_debug_integration.rs` (third test)

- [ ] **Step 1: Append the test**

```rust
#[tokio::test]
async fn skill_can_grep_jsonl() {
    let sandbox = TestSandbox::create("rightreflect-grep")
        .await
        .expect("create sandbox");

    sandbox
        .exec(&["mkdir", "-p", "/sandbox/.claude/projects/-sandbox"])
        .await
        .expect("mkdir projects");

    let marker = "RIGHTREFLECT-MARKER-XYZZY";
    let jsonl_line = format!(
        r#"{{"type":"assistant","uuid":"abc","message":{{"content":[{{"type":"tool_use","name":"mcp__right__cron_create","input":{{"job_name":"{marker}"}}}}]}}}}"#
    );
    let target = "/sandbox/.claude/projects/-sandbox/synthetic-session.jsonl";

    sandbox
        .exec(&["sh", "-c", &format!("printf '%s\\n' '{}' > {}", jsonl_line.replace('\'', "'\\''"), target)])
        .await
        .expect("write synthetic jsonl");

    let grep = sandbox
        .exec(&[
            "sh",
            "-c",
            &format!(
                "grep -l {marker} /sandbox/.claude/projects/-sandbox/*.jsonl"
            ),
        ])
        .await
        .expect("grep");

    assert!(
        grep.stdout.contains("synthetic-session.jsonl"),
        "grep didn't find marker:\nstdout={}\nstderr={}",
        grep.stdout,
        grep.stderr
    );
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p right-bot --test cc_debug_integration skill_can_grep_jsonl -- --nocapture
```

Expected: passes.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/tests/cc_debug_integration.rs
git commit -m "test(bot): grep-on-jsonl smoke test for /rightreflect mechanics"
```

---

## Task 17: Final workspace build, clippy, full test run

- [ ] **Step 1: Workspace build**

```bash
cargo build --workspace 2>&1 | tail -10
```

Expected: clean. Errors → fix, then re-run.

- [ ] **Step 2: Clippy**

```bash
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -30
```

Expected: no warnings. Per AGENTS.rust.md, fix any warning rather than allow.

- [ ] **Step 3: Full test run**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: all green.

- [ ] **Step 4: review-rust-code subagent pass (per global preference)**

Dispatch `rust-dev:review-rust-code` to audit the diff. Capture issues
as TODOs and fix one at a time before merging.

- [ ] **Step 5: Manual verification checklist**

In a real bot session:

- [ ] Restart bot fresh; send `/debug` in a private chat. Reply says debug is OFF.
- [ ] Send `/debug on`. Reply confirms ON.
- [ ] Ask the agent any trivial question.
- [ ] SSH into the sandbox and confirm:
  ```
  ls -la /sandbox/.claude/logs/
  ```
  A file matching the session UUID exists with non-zero size.
- [ ] Send `/debug` again. Reply says ON.
- [ ] Ask the agent: "Why did you answer X to my last question?". Verify the agent activates `/rightreflect`, finds the right JSONL via grep or mtime, and reports a coherent narrative referencing file paths.
- [ ] Send `/debug off`. Reply confirms OFF.
- [ ] Ask another trivial question. Confirm a NEW debug log is NOT written for the new session UUID.
- [ ] Old debug log files for the previous turn are still present (not cleaned up).

- [ ] **Step 6: Commit final fixes (if any)**

```bash
git status
# If anything was fixed during clippy/review:
git add ...
git commit -m "chore: address review feedback for self-introspection"
```

- [ ] **Step 7: Open PR**

Per global preference: do not push or open PRs unless explicitly requested. Stop here and notify the user.

---

## Self-Review Checklist (executor — for your reference)

After each task:

- Did I write a failing test first? (Skip only when the task is pure docs.)
- Does my commit message describe WHY, not just WHAT?
- Did I touch only what the task says? Refactor-creep is a deferral, not a contribution.

After Task 17:

- Spec sections all covered? Cross-reference `docs/superpowers/specs/2026-05-12-self-introspection-design.md`.
- Did I leave any TBDs in OPERATING_INSTRUCTIONS.md, ARCHITECTURE.md, SKILL.md? They should all be concrete.
- Are session-uuid lookups (Open Question #1 in spec) actually resolved? The skill teaches "ls -lt" + "most recent JSONL" — that resolves it via convention, no codegen change.
