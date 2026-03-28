---
trigger: "CC ships channels fix / iv6 M6 event loop fix"
planted: 2026-03-28
milestone: v2.4
---

# Seed: CC iv6/M6 channels fix

## Trigger Condition
When CC ships a fix for the iv6/M6() event loop gap that causes background agent SubagentStop to kill channel notification processing.

## What to Do

Fix `startup_prompt` in `crates/rightclaw/src/codegen/shell_wrapper.rs`:

Remove the "Use the Agent tool to run this in the background:" prefix so rightcron runs inline in the main thread instead of as a background sub-agent. This gives rightcron access to CronCreate (a main-thread-only built-in tool), allowing it to schedule the `*/5 * * * *` reconciler job that keeps M6() firing.

Before:
```
"Use the Agent tool to run this in the background: Run /rightcron to bootstrap the cron reconciler..."
```

After:
```
"Run /rightcron to bootstrap the cron reconciler..."
```

## Full Context
See `.planning/phases/20-diagnosis/DIAGNOSIS.md` for:
- Root cause: iv6 subscriber doesn't call M6() when Z === null after SubagentStop
- Process topology proof (socat hypothesis eliminated)
- Confirmation Test A (verify fix) and Test B (regression guard)
- cli.js event loop analysis (cc_version 2.1.86)

## Note
Once CC fixes the iv6/M6 gap, the startup_prompt fix may no longer be necessary — cron fires will drain the queue automatically even without the reconciler job. But having the reconciler is still good hygiene for rightcron's job management.
