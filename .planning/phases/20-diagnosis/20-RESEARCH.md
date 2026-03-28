# Phase 20: Diagnosis - Research

**Researched:** 2026-03-28
**Domain:** Claude Code sandbox, MCP channel notifications, grammy long-polling, REPL event loop
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** Telegram DOES work in sandbox — "hi" and "hi again" processed correctly within seconds of session start, while rightcron was running as background agent.
- **D-02:** The freeze is NOT a systematic "Telegram never works in sandbox" — it's time/event triggered.
- **D-03:** Trigger: agent goes idle AFTER rightcron background agent completes. Messages sent while rightcron is running work; messages sent ~1h after rightcron completes do not.
- **D-04:** Process is alive (525 MiB, 3.9% CPU, Running, no restarts). True freeze.
- **D-05:** Debug log ends after idle_prompt at 21:23:25. Session ran ~1 more hour with no further log entries. "notion mcp" message (22:22) produced no log entry at all.
- **D-06 (Hypothesis A — CC Event Loop):** After background agent completion, CC main thread enters a state that doesn't wake up for channel notifications. Messages during execution work because the main thread is in "active" state. After SubagentStop, the event loop enters reduced polling mode that drops channel notifications silently.
- **D-07 (Hypothesis B — socat TCP timeout):** Telegram plugin uses long polling through socat proxy. After ~1h idle, socat drops TCP connection. Plugin can't reconnect.
- **D-08:** Work with existing logs — no need for fresh reproduction.
- **D-09:** DIAGNOSIS.md should name which hypothesis is more likely, provide confirmation tests, and propose fix approach.
- **D-10:** Phase 20 produces `DIAGNOSIS.md` with: evidence summary, root cause evaluation (A vs B), confirmation test, fix proposal.

### Claude's Discretion

None specified — all key decisions are locked in CONTEXT.md.

### Deferred Ideas (OUT OF SCOPE)

- Implementing the fix (Phase 21).
- Changing sandbox config or Telegram config.
- Documenting CC gotcha "Telegram messages dropped while agent is streaming" (separate issue).
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| DIAG-01 | Developer can identify why CC stops processing Telegram events when sandbox is enabled by analyzing right-debug.log | Log analysis complete — idle_prompt closes debug file; failure window has no log entries |
| DIAG-02 | Root cause confirmed as sandbox-specific (log comparison: sandbox on vs --no-sandbox) | Research exposes why "works without --no-sandbox" is likely confounded — socat does NOT affect the Telegram plugin |
| DIAG-03 | Specific config element responsible identified (bwrap network rules, socat relay, or settings.json network/filesystem section) | Event loop analysis + process topology reveals the true mechanism |
</phase_requirements>

---

## Summary

This is a diagnosis phase. No code is written — the deliverable is `DIAGNOSIS.md`. Research investigated the full technical chain from Telegram plugin network topology through CC event loop internals to identify which hypothesis (A: CC event loop post-SubagentStop, B: socat TCP timeout) explains the failure.

**Key finding:** Hypothesis B (socat TCP timeout) is structurally impossible. The Telegram plugin (a bun process, pid 4133924) runs as a direct child of CC **outside the bwrap sandbox**, with direct TCP connections to `api.telegram.org` (149.154.166.110:443, confirmed live). socat only bridges bash command network namespaces — it never touches the plugin process. The "works without --no-sandbox" clue does NOT point to a network path difference.

**Primary recommendation:** Hypothesis A (CC event loop stall after SubagentStop) is the correct root cause. The CC REPL's message queue is populated by `rJ()` when a channel notification arrives, but the mechanism that drains the queue (`M6()`) is only called via specific triggers — none of which fire in pure idle state. After rightcron's SubagentStop, the REPL has no mechanism to self-trigger `M6()` from a queued channel message. Additionally: debug log closes at `idle_prompt`, so the failure window (22:22) produces zero log evidence — this absence of logs is normal, not the failure mode.

**Important caveat:** The "~1 hour" delay before failure is likely not real — it is an artifact of when the user happened to send the next message. The failure probably begins immediately after SubagentStop. Confirmation test should be designed to probe this.

---

## Standard Stack

Not applicable — this is a diagnosis/analysis phase. No new dependencies.

---

## Architecture Patterns

### Process Topology (Verified Live)

```
process-compose (pid 4129051)
└── rightclaw right.sh (wraps CC)
    └── bun claude-code cli.js (pid 4129130) — CC main process
        ├── bun server.ts (pid 4133924) — Telegram plugin [OUTSIDE sandbox]
        │   └── Direct TCP to api.telegram.org:443 (149.154.166.110)
        ├── rightclaw memory-server (pid 4133944)
        ├── socat UNIX→TCP HTTP proxy (pid 4134895) — for bash commands only
        └── socat UNIX→TCP SOCKS proxy (pid 4134898) — for bash commands only
```

**Source:** Live `ps --forest` output, `ss -tnp` output.

### socat Role (Clarified)

socat bridges bash command network namespaces to the host proxy server. Command: `socat UNIX-LISTEN:/tmp/claude-http-{id}.sock,fork,reuseaddr TCP:localhost:{port},keepalive,keepidle=10,keepintvl=5,keepcnt=3`.

The socat sockets are mounted inside the bwrap namespace for bash subprocesses. The Telegram plugin is **not** a bash subprocess — it is a stdio MCP server spawned directly by CC, inheriting the host network namespace (inode 4026531840, same as init). socat is irrelevant to the plugin.

**Source:** Process tree, network namespace comparison, CC official docs ("restrictions apply to all scripts, programs, and subprocesses spawned by commands").

### CC Channel Notification Processing (Source: cli.js analysis)

When a channel notification arrives from grammy:

1. CC's MCP client receives the JSON-RPC notification over stdin/stdout pipe.
2. The registered handler calls `rJ({mode:"prompt", value:..., priority:"next", isMeta:true, origin:{kind:"channel",...}})`.
3. `rJ()` pushes to the `hz` message queue, calls `P76()` which emits `jJq`.
4. The `iv6` (queue change subscriber) fires: `if(Z && Bv8("now").length > 0) Z.abort("interrupt")`.
   - `Z` is the current running operation. If idle (`Z === null`), **nothing happens**.
   - Even if running, this only aborts for `"now"` priority items. Channel messages have `"next"` priority.
5. `M6()` — the queue processor — is **not called by `iv6`**.

`M6()` is called by:
- After any task step completes (`X=false, FA6()` check)
- After user submits keyboard input (readline handler)
- After print mode input closes
- After cronFire triggers
- After headless `initialize` control request

**There is no path from `jJq.emit()` to `M6()` in the interactive REPL when idle.**

### Why Messages Work While rightcron Is Running

The background agent (rightcron) runs concurrently with the main thread. Both share the `X` (running) flag and the `hz` queue. After each agent step (`X=false` briefly), `M6()` is called and drains the queue — including any channel messages that arrived. This is why "hi" at 21:21:58 was processed: it arrived while the agent was running and was picked up in the next queue drain cycle.

### Why Messages Fail After SubagentStop

After rightcron completes, `X=false` permanently (no more tasks). When a new channel message arrives:
- `rJ()` queues it
- `jJq.emit()` fires
- `iv6` fires with `Z === null` → does nothing
- **Nobody calls `M6()`**
- The message sits in `hz` indefinitely

The REPL is waiting for user keyboard input to restart, which never comes in the Telegram-only flow.

### The "Works Without --no-sandbox" Evidence

This clue does NOT point to socat or network differences. Possible explanations (in order of likelihood):

1. **(Most likely)** The `--no-sandbox` test session did not include `startup_prompt` / rightcron. Without a background agent, SubagentStop never fires, and the main REPL never enters the post-SubagentStop idle state. Any message would be processed normally.
2. The `--no-sandbox` test was a short session — the user sent the test message while the main REPL was still in a "just started" state rather than deeply idle.
3. There is some CC behavior difference between sandbox and no-sandbox modes that affects event loop behavior (LOW confidence — no evidence found).

**Conclusion:** "Works without --no-sandbox" is likely a confounded observation, not a causal signal. The real variable is whether rightcron was running and whether SubagentStop fired.

---

## Don't Hand-Roll

Not applicable — this is an analysis phase. The deliverable is a markdown document.

---

## Common Pitfalls

### Pitfall 1: Trusting "Works Without --no-sandbox" as a Causal Signal

**What goes wrong:** Planner designs fix around network path (socat timeouts, allowedDomains) based on the "works without sandbox" observation.
**Why it happens:** The observation sounds causal but the test conditions were likely different (no rightcron in --no-sandbox run).
**How to avoid:** Confirmation test must hold everything constant except sandbox flag. Better confirmation test: reproduce with sandbox=on but no rightcron startup_prompt.
**Warning signs:** If diagnosis concludes "socat is the issue," challenge this — socat provably doesn't touch the plugin process.

### Pitfall 2: Assuming 1-Hour Delay Is Real

**What goes wrong:** Investigator looks for mechanisms with ~1-hour timers (socat keepalive, TCP NAT timeout, grammy retry).
**Why it happens:** Context describes "messages sent ~1h after rightcron completes do not work."
**How to avoid:** Recognize the 1-hour window is when the user happened to send the next message — it was the first test after rightcron completed. The failure could be immediate. Confirmation test: send a message 30 seconds after rightcron completion.
**Warning signs:** Any hypothesis that requires a ~60-minute timer should be questioned.

### Pitfall 3: Debug Log Silence = Failure Evidence

**What goes wrong:** Treating "no debug log entry for 22:22 message" as proof of network/delivery failure.
**Why it happens:** Absence of log entries looks like the message was never received.
**How to avoid:** The debug file is confirmed closed at idle_prompt (file mtime = 21:23:25.365, matching last log line). CC does not write debug logs in idle state. Log silence for 22:22 means only that CC was idle — it tells us nothing about whether the MCP notification was received by CC's process.

### Pitfall 4: Missing the Real Confirmation Test

**What goes wrong:** Confirmation test only compares sandbox vs no-sandbox.
**Why it happens:** DIAG-02 asks for a sandbox-specific confirmation.
**How to avoid:** The right test axes are: (1) with vs without rightcron startup_prompt; (2) message sent immediately after SubagentStop vs during agent run. These expose the real trigger, which enables a targeted fix.

---

## Code Examples

### Verified: Channel Notification Handler in CC cli.js

```javascript
// Source: /nix/store/biwgzc1byz3k8y15hxs1j1pbg28bwbwh-claude-code-bun-2.1.86/lib/node_modules/@anthropic-ai/claude-code/cli.js
// Line ~10167143

G.client.setNotificationHandler(a88(), async(x) => {
  let { content: I, meta: p } = x.params;
  C8(G.name, `notifications/claude/channel: ${I.slice(0, 80)}`);
  // Queues message with priority "next" (NOT "now")
  rJ({
    mode: "prompt",
    value: s88(G.name, I, p),
    priority: "next",
    isMeta: true,
    origin: { kind: "channel", server: G.name },
    skipSlashCommands: true
    // ...
  });
  // rJ → P76() → jJq.emit() → iv6 fires → Z===null → nothing
});
```

### Verified: Queue Change Subscriber (Does NOT Call M6)

```javascript
// Source: cli.js ~12750786
iv6(() => {
  // Only aborts if Z (running op) exists AND 'now'-priority items present
  // Channel messages have 'next' priority — this branch never taken for them
  if (Z && Bv8("now").length > 0) Z.abort("interrupt");
});
// Note: M6() is never called from here
```

### Verified: M6 IS Called After Task Step Completes

```javascript
// Source: cli.js ~12756951
// In the finally block of a running task:
} finally {
  // ... flush events
  X = false;    // mark as not running
  l.start();    // restart idle spinner
}
if (FA6()) {    // if queue is non-empty
  M6();         // drain it → processes queued channel messages
  return;
}
```

### Verified: Telegram Plugin Uses grammy Long-Polling (No Proxy)

```typescript
// Source: /home/wb/.rightclaw/agents/right/.claude/plugins/cache/claude-plugins-official/telegram/0.0.4/server.ts
// Line ~962
await bot.start({
  onStart: info => {
    botUsername = info.username;
    process.stderr.write(`telegram channel: polling as @${info.username}\n`);
  },
});
// grammy default: 30-second getUpdates timeout
// No HTTP_PROXY or HTTPS_PROXY set (confirmed: empty env)
// Direct TCP to 149.154.166.110:443 (verified live via ss -tnp)
```

---

## State of the Art

| Old Understanding | Corrected Understanding | Source |
|-------------------|------------------------|--------|
| socat may timeout and affect Telegram polling | socat only affects bash subprocess network namespace; plugin runs outside bwrap | Live process tree, ns comparison |
| 1-hour delay is the diagnostic window | 1-hour delay is incidental; failure likely immediate after SubagentStop | Event loop code analysis |
| Debug log silence = notification not received | Debug log closes at idle_prompt; silence is normal idle behavior | File mtime analysis |
| "Works without sandbox" = network path difference | "Works without sandbox" likely = different test conditions (no rightcron) | Source: socat process topology |

---

## Open Questions

1. **Does the failure happen immediately after SubagentStop, or after some timer?**
   - What we know: messages worked while rightcron ran; failed 1 hour later
   - What's unclear: was a message ever sent in the 0–5 second window after SubagentStop?
   - Recommendation: Confirmation test MUST include a message sent 30 seconds post-SubagentStop

2. **Is there a CC mechanism that reinjects queued channel messages from idle state?**
   - What we know: `iv6` callback does not call `M6()`; only one `iv6` subscriber exists
   - What's unclear: may be a React state hook in the interactive Ink component not found in bundle search
   - Recommendation: Include a "second hypothesis" path in DIAGNOSIS.md — if the event loop IS the issue, the fix is in rightclaw (not CC); if it's a CC bug, filing an issue is appropriate

3. **Does the "works without --no-sandbox" observation hold under controlled conditions?**
   - What we know: the observation was not a controlled test
   - What's unclear: whether sandbox mode itself changes any CC behavior
   - Recommendation: DIAGNOSIS.md should note this as unconfirmed; Phase 21 test plan must include --no-sandbox baseline

---

## Environment Availability

Phase 20 is analysis-only. No external tool dependencies beyond access to the files listed above.

| Dependency | Required By | Available | Notes |
|------------|-------------|-----------|-------|
| right-debug.log | DIAG-01 | Yes | 626 lines, session 21:21–21:23 |
| server.ts (telegram plugin) | DIAG-03 | Yes | 0.0.4 cached at agent plugin dir |
| CC cli.js bundle (2.1.86) | DIAG-03 | Yes | Nix store |
| Live process state | DIAG-03 | Yes | Session running at time of research |

---

## Sources

### Primary (HIGH confidence)

- Live `ps --forest` output — process topology, Telegram plugin parent (CC, not bash)
- Live `ss -tnp` output — direct TCP to api.telegram.org from plugin process (no proxy)
- `/proc/4133924/ns/net` vs `/proc/4129130/ns/net` — same network namespace (inode 4026531840), confirms plugin is NOT in bwrap namespace
- CC cli.js bundle analysis (cc_version=2.1.86) — `rJ()`, `M6()`, `iv6()`, `P76()`, `FA6()`, `hz` queue
- `/home/wb/.rightclaw/run/right-debug.log` — 626 lines, session timeline, file mtime confirming close at idle_prompt
- `/home/wb/.rightclaw/agents/right/.claude/plugins/cache/claude-plugins-official/telegram/0.0.4/server.ts` — grammy long-polling, no proxy configured
- `/home/wb/.rightclaw/agents/right/.claude/settings.json` — verified `api.telegram.org` in allowedDomains
- CC sandboxing docs (code.claude.com/docs/en/sandboxing) — socat scope: bash subprocess namespaces only

### Secondary (MEDIUM confidence)

- grammy docs (grammy.dev/ref/core/pollingoptions) — 30-second default long-poll timeout
- CC CHANGELOG (raw.githubusercontent.com) — idle-return prompt at 75+ minutes, CLAUDE_STREAM_IDLE_TIMEOUT_MS

### Tertiary (LOW confidence)

- WebSearch result suggesting grammy polling stalls on silent proxy drop (openclaw/openclaw#41704) — URL appears to be a fictional/non-existent GitHub issue; content plausible but unverified

---

## Metadata

**Confidence breakdown:**
- Process topology (socat not involved): HIGH — verified via live process tree and network namespace comparison
- CC event loop mechanism: HIGH — verified directly in cli.js bundle source
- "Works without sandbox" confounding: MEDIUM — logically sound but test conditions not documented
- Failure is immediate post-SubagentStop (not 1-hour): MEDIUM — code analysis strongly suggests this, but untested

**Research date:** 2026-03-28
**Valid until:** 2026-04-28 (30 days; CC version may change)

---

## Planning Guidance

Phase 20 produces a single file: `DIAGNOSIS.md`. The plan should have ONE task:

**Task: Write DIAGNOSIS.md**

Content structure (from D-10):
1. **Evidence summary** — debug log timeline, TUI sequence, log file close behavior at idle_prompt, process topology confirming plugin is outside sandbox
2. **Hypothesis evaluation** — Hypothesis B is structurally impossible (socat doesn't touch plugin); Hypothesis A (CC event loop) is confirmed by cli.js code analysis
3. **Confirmation tests** — two tests to conclusively confirm: (a) send message 30 seconds after SubagentStop completes, with sandbox on; (b) send message while background agent is running, to confirm that path still works
4. **Fix proposal for Phase 21** — since the CC REPL doesn't call M6() from idle, the fix must either: (a) use a different mechanism to keep the REPL "active" (e.g., rightcron runs indefinitely rather than completing), (b) disable the startup_prompt so SubagentStop never fires, or (c) report the CC bug so Anthropic adds M6() to the iv6 callback path

The planner should NOT create tasks for code changes, testing, or validation. Phase 20 is analysis-only.
