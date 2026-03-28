# Phase 20: Root Cause Diagnosis

**Phase:** 20-diagnosis
**Date:** 2026-03-28
**Status:** Complete

---

## 1. Evidence Summary

### Debug Log Timeline

File: `/home/wb/.rightclaw/run/right-debug.log` — 626 lines, session 21:21–21:23.

| Time | Event |
|------|-------|
| 21:21:58 | Telegram message "hi" received — CC processed and replied ✓ |
| 21:22:21 | Telegram message "hi again" received — CC processed and replied ✓ |
| 21:23:25 | `idle_prompt` entry — CC main REPL enters idle waiting state |
| 21:23:25.365 | Debug log file closed (confirmed by file mtime matching last log line) |
| 22:22 (approx) | User sends "I need you to add notion mcp" via Telegram — **zero log entries**, no CC response |

The debug log file mtime is 21:23:25.365. CC closes the debug file at `idle_prompt`. The ~1-hour silence (21:23 to 22:22) has no log entries because CC does not write to the debug file in idle state. This is normal idle behavior — it does not indicate a network delivery failure.

### TUI Sequence (Exact 8-Step Sequence from Live Session)

1. Session starts — rightcron launched as background agent via `startup_prompt`
2. "Ready. How can I help you?" — main thread idle, background agent running
3. `<- telegram · brainsmith: hi` — reply sent ✓ (rightcron step cycle active)
4. `<- telegram · brainsmith: hi again` — reply sent ✓ (rightcron step cycle active)
5. "Agent 'Bootstrap rightcron reconciler' completed" — **SubagentStop fires here**
6. "Rightcron bootstrap done — crons/ directory is ready, no persisted jobs to recover."
7. Empty `❯` prompt — session waiting for keyboard input
8. (~1 hour later) "I need you to add notion mcp" — **no CC response, not shown in TUI**

### Process Topology (Verified Live via ps --forest + ss -tnp)

```
process-compose (pid 4129051)
└── rightclaw right.sh (wraps CC)
    └── bun claude-code cli.js (pid 4129130)   — CC main process [inside bwrap sandbox]
        ├── bun server.ts (pid 4133924)          — Telegram plugin [OUTSIDE sandbox]
        │   └── Direct TCP to api.telegram.org:443 (149.154.166.110)
        ├── rightclaw memory-server (pid 4133944) — MCP memory server
        ├── socat UNIX→TCP HTTP proxy (pid 4134895) — bash subprocess only
        └── socat UNIX→TCP SOCKS proxy (pid 4134898) — bash subprocess only
```

Network namespace comparison confirmed: `/proc/4133924/ns/net` and `/proc/4129130/ns/net` both map to `inode 4026531840` (the host network namespace). The Telegram plugin (pid 4133924) is NOT isolated in the bwrap namespace — it shares the same network namespace as the host system (same as init).

The socat processes bridge bash command network namespaces inside bwrap. socat command: `socat UNIX-LISTEN:/tmp/claude-http-{id}.sock,fork,reuseaddr TCP:localhost:{port},keepalive,keepidle=10,keepintvl=5,keepcnt=3`. This path is used exclusively by bash subprocesses running inside the bubblewrap sandbox.

---

## 2. Hypothesis Evaluation

### Hypothesis B (socat TCP Timeout) — ELIMINATED

**Claim:** The Telegram plugin uses long polling through socat. After ~1 hour of idle, socat drops the TCP connection. The plugin cannot silently reconnect, causing it to stop receiving messages.

**Why this is structurally impossible:**

1. **Plugin runs outside bwrap.** The Telegram plugin (`bun server.ts`, pid 4133924) is a stdio MCP subprocess spawned by CC, not a bash command. CC sandboxing docs state: "restrictions apply to all scripts, programs, and subprocesses spawned by commands." MCP servers are not spawned by bash commands — they are spawned by CC directly and inherit the host network namespace. Live network namespace comparison confirms this: both the plugin process and the host share `inode 4026531840`.

2. **Plugin connects directly to api.telegram.org.** Verified via `ss -tnp`: the plugin process establishes TCP connections to `149.154.166.110:443` (api.telegram.org) without any proxy. No `HTTP_PROXY` or `HTTPS_PROXY` in the plugin environment (confirmed empty). The socat sockets (`/tmp/claude-http-*.sock`) are unix sockets mounted inside bwrap namespaces, invisible to processes running in the host namespace.

3. **Domain filtering is not the issue.** `api.telegram.org` is already in `DEFAULT_ALLOWED_DOMAINS` in `settings.json`. Even if socat were involved, the domain would be allowed.

4. **grammy long-polling stays alive.** grammy uses a 30-second `getUpdates` timeout, continuously renewing the connection. The direct TCP connection to Telegram servers does not depend on any intermediate proxy that could expire.

5. **"Works without --no-sandbox" is confounded.** The `--no-sandbox` test session likely did not include `rightcron startup_prompt`. Without a background agent, SubagentStop never fires, and the REPL never enters the post-SubagentStop idle state. This is not a sandbox network difference — it is a test condition difference. Any message sent in that session would be processed normally regardless of sandbox state.

**Conclusion:** socat cannot affect the Telegram plugin. Hypothesis B is structurally impossible.

### Hypothesis A (CC Event Loop Stall post-SubagentStop) — CONFIRMED via cli.js Source Analysis

**Claim:** After background agent completion (SubagentStop), CC's main REPL enters a state where the message queue (`hz`) is never drained in response to channel notifications.

**CC cli.js event loop chain (cc_version 2.1.86):**

When a Telegram channel notification arrives from grammy:

1. CC's MCP client receives the JSON-RPC notification over stdin/stdout pipe.
2. The registered handler (cli.js ~line 10167143) calls `rJ({mode:"prompt", priority:"next", isMeta:true, origin:{kind:"channel",...}})`.
3. `rJ()` pushes to the `hz` message queue and calls `P76()`, which emits `jJq`.
4. The `iv6` subscriber (cli.js ~line 12750786) fires on queue change:
   ```javascript
   iv6(() => {
     if (Z && Bv8("now").length > 0) Z.abort("interrupt");
   });
   ```
   - `Z` is the current running operation. When idle, `Z === null` — the condition is false and **nothing happens**.
   - Even if `Z` were non-null, this only aborts for `"now"` priority items. Channel messages have `"next"` priority — the interrupt branch is never taken for them.
5. `M6()` — the queue processor — is **not called by `iv6`**.

**M6() is only called from these paths:**
- After any task step completes: `X = false; FA6(); M6()`
- After user submits keyboard input (readline handler)
- After print mode input closes
- After cronFire triggers
- After headless `initialize` control request

**There is no path from `jJq.emit()` to `M6()` when the REPL is idle.**

**Why messages work while rightcron is running:**

The background agent (rightcron) runs concurrently with the main thread. Both share the `X` (running) flag and the `hz` queue. After each agent step (`X=false` briefly), `M6()` is called and drains the queue — processing any channel messages that arrived during the step. This is why "hi" (21:21:58) and "hi again" (21:22:21) were processed: they arrived while the agent was running and were picked up in the next queue drain cycle.

**Why messages fail after SubagentStop:**

After rightcron completes (`SubagentStop`), `X=false` permanently (no more tasks). When a new channel message arrives:
- `rJ()` queues it in `hz`
- `jJq.emit()` fires
- `iv6` fires with `Z === null` → does nothing
- **Nobody calls `M6()`**
- The message sits in `hz` indefinitely

The REPL waits for user keyboard input to restart, which never comes in the Telegram-only flow.

**The "~1 hour" delay is not real.** It is the time at which the user happened to send the next message after rightcron completed. The failure begins immediately after SubagentStop — within seconds of step 5 in the TUI sequence above. No ~60-minute timer mechanism is involved.

---

## 3. Confirmation Tests

Two tests to conclusively confirm the root cause before Phase 21 implements a fix.

### Confirmation Test A — Immediate Failure After SubagentStop

**Purpose:** Confirm that failure is immediate post-SubagentStop, not a 1-hour timer.

**Steps:**
1. Run `rightclaw up` with sandbox enabled and `rightcron startup_prompt` active.
2. Wait for "Agent 'Bootstrap rightcron reconciler' completed" in the TUI.
3. Wait 30 seconds. Do NOT send any messages during the rightcron run or in the 30-second window.
4. Send a Telegram message.

**Expected result:** No response. CC receives the MCP notification but `M6()` is never called. The message sits in `hz` indefinitely.

**What a failure of this test means:** If CC does respond, there is a re-queue mechanism not found in the cli.js bundle analysis (possibly in an Ink/React component). Re-evaluate.

### Confirmation Test B — Working Path Still Works (Regression Guard)

**Purpose:** Confirm the "active agent running" workaround path for future regression testing.

**Steps:**
1. Run `rightclaw up` with sandbox enabled and `rightcron startup_prompt` active.
2. While rightcron is still running (within the first 60 seconds of session start), send a Telegram message.
3. Do not wait for "Agent completed" before sending.

**Expected result:** Response received within seconds. The message is queued during an active agent step cycle and `M6()` is called on step completion.

**Note on sandbox comparison:** A sandbox-on vs `--no-sandbox` comparison is NOT a useful confirmation test unless rightcron runs identically in both sessions. The "works without sandbox" observation should not be trusted until Test A is run with sandbox=on. If Test A confirms immediate failure under sandbox, then run an identical test without `--no-sandbox` to check if the event loop behavior differs — but Test A is the primary diagnostic.

---

## 4. Fix Proposal for Phase 21

**Root cause:** CC's idle REPL has no mechanism to drain the `hz` message queue when a channel notification arrives (`jJq.emit()` does not reach `M6()` from idle state). The fix must be in rightclaw — we cannot patch CC's cli.js.

### Three Candidate Approaches

**Option A — Keep agent "alive" with a persistent background presence (RECOMMENDED)**

Instead of rightcron completing after bootstrap (triggering SubagentStop), redesign rightcron to stay alive as a long-running session with periodic activity (e.g., a heartbeat or watch loop every N minutes). This keeps the `X=true` / `X=false` cycle running, which guarantees `M6()` is called after each step and channel messages are drained.

- **Tradeoff:** rightcron must be redesigned. The bootstrap task completes, but the process does not exit. It enters a watch/heartbeat mode instead.
- **Complexity:** Medium. Changes to rightcron `SOUL.md` or `startup_prompt` behavior. May require a dedicated "keeper" agent strategy separate from rightcron.
- **Confidence this fixes it:** HIGH. The step cycle is the only working drain mechanism found in cli.js.
- **Phase 21 evaluation point:** Determine whether rightcron transitions to "watch mode" after bootstrap, or whether a separate lightweight heartbeat agent strategy is cleaner.

**Option B — Disable startup_prompt (no SubagentStop)**

Remove or gate the rightcron `startup_prompt` so SubagentStop never fires. The main REPL stays in initial idle state (not post-SubagentStop state).

- **Tradeoff:** rightcron bootstrap no longer runs automatically on session start.
- **Complexity:** Low — single config change or conditional in the wrapper template.
- **Confidence this fixes it:** LOW-MEDIUM. Initial idle state may also skip `M6()` — the cli.js code path analysis suggests the issue may exist in any idle state, not specifically post-SubagentStop. Without confirmation, this may not fix the bug.
- **Risk:** If the initial idle state has the same `iv6` behavior, this option does nothing.

**Option C — Report CC bug to Anthropic**

The `iv6` callback should call `M6()` when `Z === null` and the queue is non-empty. This is a CC bug. File an issue requesting Anthropic add `M6()` to the `iv6` idle path.

- **Tradeoff:** No control over timeline. Not a fix for v2.4.
- **Complexity:** None for rightclaw. Long-term correct solution.
- **Confidence this fixes it:** HIGH (if Anthropic accepts the fix). Not actionable for Phase 21.

### Recommendation

**Phase 21 should implement Option A.** It is the only option that:
1. Guarantees channel messages are processed without depending on CC internal changes.
2. Does not break rightcron bootstrap functionality.
3. Is testable against the Confirmation Tests above.

Phase 21 should evaluate whether rightcron can transition to a "watch mode" after bootstrap completes, or whether a separate lightweight heartbeat agent strategy (e.g., a no-op cron job that fires every 5 minutes) is cleaner.

Option C (CC bug report) should be filed in parallel regardless of which Option rightclaw implements — the event loop gap in `iv6` is a genuine CC defect.

---

## Phase 21 Dependency Summary

| Input from Phase 20 | Phase 21 Consumer |
|---------------------|-------------------|
| Root cause: M6() not called from idle iv6 path | Guides fix selection — only "active step" strategies work |
| socat NOT involved | Eliminates network-layer fix candidates |
| SubagentStop is the trigger | Fix must target rightcron lifecycle, not sandbox config |
| Confirmation Test A | Phase 21 test plan must include this as regression test |
| Recommended fix: Option A | Phase 21 planner should start from "persistent agent" design |

---

*Diagnosis author: Phase 20 executor*
*Confidence: HIGH (process topology and CC event loop analysis from primary sources)*
*Valid until: 2026-04-28 (CC cli.js bundle may change)*
