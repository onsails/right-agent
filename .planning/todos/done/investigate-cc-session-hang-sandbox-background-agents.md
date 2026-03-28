---
id: investigate-cc-session-hang-sandbox-background-agents
title: Investigate CC session hang with sandbox + background agents
area: debugging
status: pending
priority: high
created: 2026-03-28
---

## Problem

When sandbox is enabled and a background agent (rightcron bootstrap) completes, the CC session stops processing events entirely. The debug log freezes after the background agent finishes — no further entries, no response to Telegram messages, no idle_prompt notifications. The process is still alive but the session is effectively dead.

Observed:
- rightcron runs as background Agent (as intended)
- Background agent completes successfully
- CC session freezes at that point
- Telegram messages arrive (bot queue is empty = polled) but CC never logs `notifications/claude/channel`
- Debug log last entry: idle_prompt at 17:09, then nothing
- Works with `--no-sandbox` flag

Reproduced on: Linux with bubblewrap sandbox enabled

## Solution

Investigate whether sandbox + background agents interact badly:
- Check if background agent subprocess holds a lock or resource that blocks the main CC event loop
- Check if bubblewrap wraps the background agent and blocks its completion signal
- Compare behavior with `--no-sandbox` to confirm it's sandbox-specific
- Look at CC source or issue tracker for background agent + sandbox interactions

## Files

- `crates/rightclaw/src/codegen/shell_wrapper.rs` — startup_prompt (background agent change)
- `/home/wb/.rightclaw/run/right-debug.log` — frozen log evidence
