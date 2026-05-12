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
