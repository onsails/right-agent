# RightClaw Harness Migration: `claude -p` CLI → Agent SDK vs ACP vs Status Quo

> **CORRECTION (2026-06-05) — read first.** Rounds 3–4 below repeat a claim that "Anthropic blocked third-party Claude subscription OAuth on 2026-01-09." **That claim is WRONG** (it came from a low-quality source in round 3's verify phase and contradicted round 1). The accurate picture, confirmed against primary sources: Anthropic **reinstated** third-party agent usage on Claude subscriptions, and from **15 June 2026** programmatic usage (Agent SDK, `claude -p`, GitHub Actions, **and third-party apps built on the Agent SDK**) draws from a **separate monthly "Agent SDK credit" pool — $20 Pro / $100 Max5x / $200 Max20x, at full API rates, per-user, no rollover** ([The New Stack](https://thenewstack.io/anthropic-agent-sdk-credits/), [Claude Help Center](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan), [VentureBeat](https://venturebeat.com/technology/anthropic-reinstates-openclaw-and-third-party-agent-usage-on-claude-subscriptions-with-a-catch)). So programmatic subscription usage is **supported and metered, not blocked**. Wherever rounds 3–4 say "subscription OAuth blocked / economics threatened by the block," substitute this. See **Round 5** for the corrected decision and the rig + subscription-OAuth architecture.
>
> **DECISION (2026-06-05):** both round-4 deciding variables are now committed by the user — **self-host is a firm requirement** and **subscription is a firm requirement** (to be satisfied via the supported programmatic-subscription credit path, with custom auth added in a rig fork if needed). Per round 4's own logic, this selects **rig** as the harness. Round 5 finalizes the subscription-OAuth feasibility and architecture.


> Decision report for a Rust engineer who knows RightClaw but hasn't seen this research. Evidence-graded; every load-bearing claim carries a primary-source URL. Where verification corrected a finding, the corrected statement is used and the contradiction is noted. Unknowns are flagged explicitly, never papered over. **Read §6 before acting: this recommendation is contingent, not unconditional.**

---

## 1. TL;DR recommendation

**Recommend A — the Claude Agent SDK (TypeScript), driven from a *Node 18+* TS sidecar running *inside* the OpenShell sandbox — pending a mandatory spike. Confidence: medium, contingent on the spike resolving the four decision-gating unknowns in §6 (especially session-UUID imposition and the `--tools ""` disable-all equivalent). Do not commit until those resolve green.**

The headline confidence is **medium, not high**, because two of the unknowns are cost-critical and recovery-critical (prompt-cache parity for a `systemPrompt` *string*, and whether `query()` accepts a caller-supplied session UUID), and the original draft's "high confidence on capability parity" contradicted its own risk list. Two of the unknowns the prior draft deferred (caller-supplied session UUID; the disable-all-tools equivalent) are partially answerable *now* from the published `sdk.d.ts` and docs — and the answers are not reassuring, so they are surfaced in the body below rather than left to a spike.

With that caveat stated, A is still the right direction. It is the only option that delivers all three "more control" asks *in-process* (tool interception, programmatic hooks, custom/gateway model routing) while preserving the riskiest features RightClaw depends on — **session forking** (`resume + forkSession`, [sessions docs](https://code.claude.com/docs/en/agent-sdk/sessions)) and **structured output** (`outputFormat: json_schema`, [structured-outputs docs](https://code.claude.com/docs/en/agent-sdk/structured-outputs)). The Agent SDK *is* the Claude Code engine — it spawns the same native binary as a subprocess ([overview](https://code.claude.com/docs/en/agent-sdk/overview), [CHANGELOG v0.2.113](https://github.com/anthropics/claude-agent-sdk-typescript/blob/main/CHANGELOG.md)) — so prompt caching, skills, and OAuth-token auth carry over in principle.

**ACP is rejected.** Its "pure Rust, no sidecar" premise is false for driving Claude Code: the official adapter is itself a Node process built on the Agent SDK ([confirmed](https://raw.githubusercontent.com/agentclientprotocol/claude-agent-acp/main/package.json)), and the protocol has **no surface** for json-schema output, forking, budget caps, max-turns, or system-prompt files, nor any input-rewrite interception ([schema](https://raw.githubusercontent.com/agentclientprotocol/agent-client-protocol/main/schema/schema.json), [protocol/schema](https://agentclientprotocol.com/protocol/schema)). So ACP gives *less* control than today while *still* requiring a Node sidecar — strictly dominated by A. (One earlier ACP finding was corrected: ACP *does* have model selection, but only as an agent-advertised dropdown, not arbitrary passthrough.)

**Staying on C (`claude -p`)** is viable — it works today and forfeits nothing — but it permanently forecloses programmatic interception, the entire point of this exercise.

**The one-paragraph why-A:** A is the only path that is a (near-)superset of today's control surface, colocates the loop and tools where the box already trusts them, and reuses OAuth-token injection. Its costs are operational (a JS runtime + correct native binary in the image, pre-1.0 SDK churn, an unanalyzed host↔sidecar trust boundary), not architectural — and they are real enough that the spike gate is non-negotiable.

---

## 2. Feature-parity matrix

Legend: ✅ supported · 🟡 partial (works with a caveat) · ❌ unsupported · ❓ unknown/unverified.

| Feature (from inventory) | `claude -p` (C) | Agent SDK (A) | ACP (B) | Killer caveat |
|---|---|---|---|---|
| `--resume` (session resume) | ✅ | ✅ `options.resume` | 🟡 | A confirmed; session_id readable from init/result ([cost-tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking)). ACP has load/resume reserved, no fork. |
| **`--session-id` (caller-dictated UUID)** | ✅ | ❓→leaning ❌ | 🟡 | **Re-graded from the draft's 🟡. The published `sdk.d.ts` confirms `resume`, `resumeSessionAt`, `forkSession`, `model`, `env`, `maxBudgetUsd` — but NO verdict confirms a *settable input* session id; `session_id` is only documented as *read back*. If unsettable, this is a real parity gap, not "needs rework." See §5 for the mutex/recovery consequences.** |
| **`--fork-session` (forking)** | ✅ | ✅ `resume + forkSession` | ❌ | Highest-risk feature; A passes. Fork branches *history only, not filesystem*; forked **Bash** edits hit the shared sandbox tree — checkpointing covers Write/Edit/NotebookEdit, **not Bash** ([sessions, confirmed](https://code.claude.com/docs/en/agent-sdk/sessions)). |
| `--mcp-config` + `--strict-mcp-config` | ✅ | ✅ `mcpServers` + `strictMcpConfig` | 🟡 | A replaces the *file* with the `mcpServers` option (Record of stdio/sse/http/sdk) ([ts ref](https://code.claude.com/docs/en/agent-sdk/typescript)). In-process tools via `createSdkMcpServer` are a bonus. |
| **`--json-schema` (structured output)** | ✅ | ✅ `outputFormat:{type:'json_schema'}` | ❌ | A passes. SDK validates + re-prompts; success carries `structured_output`, exhaustion → result subtype `error_max_structured_output_retries` ([confirmed](https://code.claude.com/docs/en/agent-sdk/structured-outputs)). **Behavioral change:** error path is a result *subtype*, not CLI's `error_max_turns` — parsing must branch on subtype. |
| `--output-format stream-json` + `--verbose` | ✅ | ✅ async iterator (`includePartialMessages`) | 🟡 | A yields typed `SDKMessage` objects; you re-serialize to NDJSON for host logs (already done today). ❓ exact event-shape equivalence not bench-verified. |
| `--model` (per-invocation) | ✅ | ✅ `model: string` + `fallbackModel` | 🟡 | A confirmed against published `sdk.d.ts`. ACP model selection is an agent-advertised dropdown, not arbitrary passthrough ([schema, partial](https://raw.githubusercontent.com/agentclientprotocol/agent-client-protocol/main/schema/schema.json)). |
| Multi-model pipeline (Haiku/Opus/curator) | ✅ separate calls | ✅ | 🟡 | A: separate `query()` calls each report own cost; `modelUsage` aggregates *within* a query, not across ([cost-tracking, corrected](https://code.claude.com/docs/en/agent-sdk/cost-tracking)). Workable. |
| `--max-budget-usd` (cron cap) | ✅ | ✅ `maxBudgetUsd` → `error_max_budget_usd` | ❌ | A: a **client-side estimate, not billing** — same as the CLI today ([confirmed](https://code.claude.com/docs/en/agent-sdk/cost-tracking)). **Lateral, not an improvement** (see §4). |
| `--max-turns` | ✅ | ✅ `maxTurns` → `error_max_turns` | ❌ | A confirmed. ACP no surface. |
| `--disallowedTools` | ✅ | ✅ `disallowedTools` | 🟡 | Semantics carry over: bare deny removes from context; scoped deny (`Bash(rm *)`) blocks even under bypass ([permissions, confirmed](https://code.claude.com/docs/en/agent-sdk/permissions)). |
| `--allowedTools` | ✅ | 🟡 | 🟡 | **Trap:** `allowedTools` *auto-approves*, it does **not restrict**. `allowedTools:[]` ≠ a tool blocklist. For probe-writer/curator's "allow only these" model you must combine allow-list with disallow or use a restrict mechanism — see the `--tools ""` row. |
| **`--tools ""` (disable ALL tools)** | ✅ (prefilter, idle-compaction) | ❓→likely needs a different option | ❓ | **Unaddressed parity gap, surfaced now.** `allowedTools:[]` does NOT reproduce disable-all (allow-list auto-approves, doesn't restrict — [confirmed](https://code.claude.com/docs/en/agent-sdk/permissions)). The CLI's restrict mechanism is `--tools`; the SDK equivalent (an option that *limits the available tool set* to empty, incl. MCP/harness) was **not identified in the evidence**. Two callsites depend on this. **Resolve from `sdk.d.ts` before committing.** |
| `--dangerously-skip-permissions` | ✅ | ✅ `permissionMode:'bypassPermissions'` | 🟡 | **Mandatory for RightClaw** (security contract). Under bypass, `canUseTool` is **never reached** — only PreToolUse hooks + scoped deny rules intercept ([confirmed](https://code.claude.com/docs/en/agent-sdk/permissions)). See §3.1: this makes `canUseTool` effectively dead for this platform. |
| `--no-session-persistence` (keepalive) | ✅ | ✅ `persistSession:false` (per verdicts) | ❓ | Minor; the draft's "all invariants map cleanly" had skipped this flag. Maps to `persistSession`. |
| Session locking (per-session mutex) | ✅ (your code) | 🟡 | 🟡 | **Re-graded from "unaffected."** Today the mutex is keyed on `root_session_id` *before spawn*. If A can't pre-assign the UUID, you must spawn-then-read-then-lock, reintroducing the TOCTOU race the mutex closes. Coupled to the `--session-id` row. |
| Reflection (failure re-entry) | ✅ | ✅ | 🟡 | A: resume + short turn + schema. Expressible. |
| Idle compaction (`/compact`) | ✅ exit-status judged, abortable via SIGKILL | 🟡 | ❓ | **Regression risk.** No programmatic `/compact` trigger found; only the `PreCompact` *hook* ([hooks](https://code.claude.com/docs/en/agent-sdk/hooks)). Sending `/compact` as a prompt string *may* work, but the "no schema, no MCP, exit-status-judged, **abortable**" contract is unverified — incl. whether aborting `query()` SIGKILLs the subprocess group cleanly. **Must spike.** |
| Learning prefilter (Haiku, no session, disable-all) | ✅ | 🟡 | 🟡 | Depends on the `--tools ""` equivalent (see that row) + json output + `model`. |
| Learning probe-writer fork | ✅ | ✅ | ❌ | Forking-dependent (A passes, ACP fails). See §3.3 for the bypass+fork+Bash interaction. |
| Learning curator (fresh session) | ✅ | ✅ | 🟡 | Fresh `query()` with allow-list (plus restrict, per the `--tools` caveat). |
| Non-foreground registration (aggregator) | ✅ (your code) | ✅ (your code) | ✅ (your code) | Platform mechanism, not a harness flag. Survives any choice; *may* simplify with in-process `mcpServers` (optional refactor, see §5). |
| Background continuation (async handoff) | ✅ | ✅ (fork) | ❌ | Forking-dependent. |
| Async delivery (relay) | ✅ | ✅ resume + json schema | 🟡 | |
| Cron job invocation | ✅ | ✅ | 🟡 | Budget cap needs A (ACP lacks it). |
| Debug flag → `--debug --debug-file` | ✅ | ❓ | ❓ | SDK equivalent of `--debug-file` path not verified. Low stakes; likely env/stderr capture. |
| Auth via `CLAUDE_CODE_OAUTH_TOKEN` (env) | ✅ | ✅ via `env` option | ✅ via env block | A: `env` **replaces** subprocess env — spread `{...process.env}` to keep PATH/HTTPS_PROXY, and **never include `ANTHROPIC_API_KEY`** (rank #3 outranks OAuth #5) ([confirmed](https://code.claude.com/docs/en/authentication)). |
| Sandbox exec contract (OpenShell) | ✅ (your code) | 🟡 | 🟡 | See §4 — the guard shifts from "build remote argv" to "host↔in-sandbox-sidecar liveness," which is a **new trust boundary** the report does not consider solved. |

**Headline takeaways:** (1) The two riskiest features — **forking** and **structured output** — are confirmed in A and confirmed absent in ACP. (2) ACP is `❌` on *five* load-bearing features while still needing a Node process — not a serious contender. (3) **Two parity items the prior draft under-graded are surfaced as open gaps: caller-dictated session UUID and disable-all-tools.** Both are answerable from `sdk.d.ts` and must be resolved before committing.

---

## 3. The three "more control" asks

### 3.1 Tool-call interception (inspect / allow / deny / **modify**)

- **Agent SDK (A): delivered, but narrowed by the bypass constraint.** Two mechanisms exist:
  - `canUseTool(toolName, input, options) → {behavior:'allow', updatedInput?} | {behavior:'deny', message}` — verified against the published `sdk.d.ts` of `@anthropic-ai/claude-agent-sdk@0.3.162` ([confirmed](https://code.claude.com/docs/en/agent-sdk/typescript)). `updatedInput` rewrites input before execution; Claude is not told ([user-input, confirmed](https://code.claude.com/docs/en/agent-sdk/user-input)).
  - PreToolUse/PostToolUse hooks, which can also rewrite via `updatedInput` / `updatedToolOutput`.
  - **The decisive constraint for RightClaw:** `--dangerously-skip-permissions` is a *stated security contract* ("OpenShell policy is the security layer, not CC's permission system" — ARCHITECTURE.md). The SDK equivalent is `permissionMode:'bypassPermissions'`, which is therefore **mandatory**. Under bypass, the permission-mode step approves everything *before* the `canUseTool` step is reached ([confirmed](https://code.claude.com/docs/en/agent-sdk/permissions)), so **`canUseTool` never fires**. The draft framed "keep bypass + hooks" vs "switch to `default` + canUseTool" as a neutral design choice. It is not: switching to `default` mode reintroduces CC's permission system as a security layer, directly against the documented model. **Therefore only the hooks path is compatible. All programmatic interception MUST be expressed as PreToolUse hooks + scoped deny rules; `canUseTool` is effectively dead for this platform.** This narrows the "more control" win to what hooks can do (which is substantial — deny is authoritative even under bypass, and PreToolUse `updatedInput` rewrites input), but it should be stated plainly, not sold as the full `canUseTool` surface.
  - Hooks cover built-in **and** MCP tools (the `^mcp__` matcher and unified flow — [mcp, confirmed](https://code.claude.com/docs/en/agent-sdk/mcp)).
- **ACP (B): allow/deny only, no rewrite.** `session/request_permission` returns a selected `optionId` or `Cancelled` — no field for modified input ([confirmed](https://agentclientprotocol.com/protocol/schema)). The "updatedInput" some sources attribute to ACP is the Agent SDK PreToolUse hook leaking through the adapter, not an ACP feature. Strictly weaker than today + the SDK.
- **CLI (C): none programmatically.** Only shell hooks from settings files.

### 3.2 Programmatic hooks (your code, not shell scripts)

- **Agent SDK (A): delivered exactly as asked, and this is the load-bearing interception surface.** Hooks are in-process JS/TS callbacks in `options.hooks` (typed `Partial<Record<HookEvent, HookCallbackMatcher[]>>`), distinct from settings-file shell hooks ([confirmed](https://code.claude.com/docs/en/agent-sdk/hooks)). The TS SDK exposes a strict superset of Python events: PreToolUse, PostToolUse, PostToolUseFailure, PostToolBatch, UserPromptSubmit, Stop, SubagentStart/Stop, PreCompact, PermissionRequest, SessionStart/End, plus TS-only events. PreToolUse sets `permissionDecision` (allow/deny/ask/defer) + `updatedInput`; PostToolUse sets `additionalContext` or `updatedToolOutput`. **Hook `deny` is authoritative across all modes including bypass** ([confirmed](https://code.claude.com/docs/en/agent-sdk/hooks)) — this is the security-relevant property that makes bypass-mode interception viable at all.
- **ACP (B):** no programmatic hook system in the protocol.
- **CLI (C):** only shell-command hooks from settings files — out-of-process, no rich return values.

### 3.3 The fork + bypass + Bash interaction (the unexamined corner)

This connects two findings the draft kept separate, because together they bound the "more control" promise at the one place RightClaw does uncontrolled filesystem mutation.

The probe-writer fork runs with `--allowedTools Write,Read,Bash,skill_learning_*`. Under the mandatory `bypassPermissions` mode:
- Interception of the forked agent's `Bash` calls is **hooks-only** (`canUseTool` is dead, per §3.1).
- Forked **Bash** edits hit the shared sandbox tree, and file checkpointing covers Write/Edit/NotebookEdit but **not Bash** ([sessions, confirmed](https://code.claude.com/docs/en/agent-sdk/sessions)).

So the one place forks perform uncontrolled filesystem mutation is also the place input-rewrite is weakest. The "more control" win is real for built-in editing tools and MCP tools (PreToolUse hooks rewrite/deny them), but does **not** give you fine-grained control of *what the probe-writer's Bash does* beyond a coarse PreToolUse allow/deny/rewrite-the-command-string on the `Bash` tool. This is no worse than today's CLI (which also runs bypass + can't intercept Bash mutations in-process), but it is **not the upgrade the headline implies** for that path. State it to stakeholders.

### 3.4 Fine-tuned / custom models

The most over-promised area; corrected evidence matters.

- **No self-service first-party fine-tuning.** The official glossary states "Our API does not currently offer fine-tuning" ([confirmed](https://platform.claude.com/docs/en/about-claude/glossary)). The only documented managed path is **Amazon Bedrock for Claude 3 Haiku** (GA Nov 1 2024, US West Oregon, text-only, ≤32K context — [confirmed](https://aws.amazon.com/about-aws/whats-new/2024/11/fine-tuning-anthropics-claude-3-haiku-amazon-bedrock/)).
- **No primary source confirms fine-tuning of newer families** (Haiku 4.5, Sonnet, Opus) on Bedrock; the Haiku 4.5 model card lists no customization capability — treat as **unavailable, not confirmed** ([confirmed](https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-anthropic-claude-haiku-4-5.html)). Claude fine-tuning on Vertex is unconfirmed. ❓
- **Routing is mostly harness-agnostic** because all three run the same engine and honor the same env/settings contract: point `ANTHROPIC_BASE_URL` at your existing gateway (changes *where*, not *which model* — [confirmed](https://code.claude.com/docs/en/model-config)) and set the model id (a Bedrock custom-model ARN or gateway model id) via `ANTHROPIC_MODEL` / `ANTHROPIC_DEFAULT_*_MODEL` / `modelOverrides` ([confirmed](https://code.claude.com/docs/en/model-config)).
- **But A is more reliable than ACP for model control** (corrected from "no difference"): the SDK adds programmatic overrides — a `model` query option, runtime `setModel()`, `settingSources` to disable settings.json — and the **official ACP adapter has a documented bug where `ANTHROPIC_MODEL`/`settings.json` are ignored and the model is forced to a default** ([partial](https://code.claude.com/docs/en/agent-sdk/typescript)). The SDK's explicit `model` option is the safest path.

**Bottom line:** "fine-tuned models" in practice means *gateway/Bedrock-routed custom deployments*, and A gives the most reliable, explicit control. Genuine fine-tuning of the models RightClaw actually uses (Opus/Haiku 4.5) is **not currently available** through any of these paths — flag this expectation to stakeholders.

---

## 4. Runtime & security analysis

### Where the loop runs

The Agent SDK runs an in-process *orchestration* loop and **spawns the native Claude Code binary as a subprocess** over stream-json stdio ([confirmed against published `sdk.mjs`](https://code.claude.com/docs/en/agent-sdk/typescript)). For RightClaw this means **option A = a Node 18+ TS sidecar inside the sandbox** that spawns the binary locally. Loop + tools + binary all execute *inside the box* — the security model ("Claude Code runs entirely inside the box") is preserved *in direction*. The caveats below qualify "preserved."

**Runtime: Node 18+, not Bun.** The published `engines` floor is **Node ≥18** ([npm, confirmed](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/latest)); there is **no documented Bun minimum**, and Bun's in-sandbox `child_process.spawn` of the native binary is **unverified** (open question #8). The old Bun-vs-Node crash (#266) is moot on current SDKs (fixed v0.2.51, cause eliminated by the v0.2.113 native-binary migration — [partial](https://github.com/anthropics/claude-agent-sdk-typescript/issues/266)), so it should *not* drive a Bun decision either way. **The evidence supports Node 18+ as the baseline; Bun is an unproven optimization.** (The prior draft's "Bun/TS" framing in title and body was optimism leaking past the evidence.)

The "no sidecar from Rust via ACP" idea is dead:
- The official ACP adapter is a **Node process** built on the Agent SDK ([confirmed](https://raw.githubusercontent.com/agentclientprotocol/claude-agent-acp/main/package.json)).
- The leading Rust-ACP project (`claude-code-rust`) **abandoned ACP in v0.4.0** and now bridges the Agent SDK via its own Node child process ([partial — the "spawns claude-code-acp" claim is stale](https://github.com/srothgan/claude-code-rust/blob/main/CHANGELOG.md)).
- The one pure-Rust ACP agent (`claude-code-acp-rs`) does **not** reimplement the loop (that finding was **refuted**) — it spawns the *real* `claude` CLI subprocess, re-wrapping the very CLI you're leaving, with documented **API-key-first auth and no documented `CLAUDE_CODE_OAUTH_TOKEN`/subscription support** ([refuted](https://github.com/soddygo/claude-code-acp-rs/blob/main/Cargo.toml)) — a hard blocker given per-agent OAuth injection.

**B2 (ACP loop on the host) is the worst option** — it moves the loop *out* of the sandbox, defeating isolation. Do not consider it.

### Host↔sidecar trust boundary (new, unanalyzed — the draft's biggest security gap)

The "security preserved" claim rests on the sidecar running inside the sandbox and spawning the binary locally, which sidesteps SSH for the agent turn. But it introduces a **new question the draft answered with one clause: how does the host Rust process talk to the in-sandbox sidecar?** This is the single largest new attack surface and the report does **not** treat it as solved:

- If the sidecar binds a socket inside the sandbox, the host reaches it through OpenShell network policy — a **new ingress endpoint into the sandbox** that ARCHITECTURE.md's policy model must explicitly encode. The draft's "TLS-MITM unaffected / fail-closed re-expressed" does not cover host→sidecar ingress at all. This needs **design**, not assertion.
- The `guard_no_sandboxed_host_exec` re-expression ("refuse to talk to the sidecar when unreachable") risks violating an explicit project rule: AGENTS.md mandates "direct, observable signals over indirect heuristics." Treating *socket-connect success* as "sidecar healthy" is exactly the SSH-connectivity-as-readiness anti-pattern the project forbids. **The report must specify what direct health signal the sidecar exposes** (e.g. the SDK `startup()` initialize-handshake result, or an explicit health RPC) rather than inferring liveness from connectivity. This is an open design item, listed in §6.

So the honest security story is: **"preserved IF a new ingress + direct-health-signal design holds up,"** not "preserved, with a footgun."

### TLS-MITM / credential isolation / fail-closed

- **TLS-MITM:** unaffected. The spawned binary still routes all HTTP/HTTPS through `HTTPS_PROXY=http://10.200.0.1:3128`; the SDK doesn't bypass the proxy. Provider placeholder substitution at the proxy is orthogonal.
- **Credential isolation:** carries over with a new footgun. `CLAUDE_CODE_OAUTH_TOKEN` is injected via the SDK `env` option, which **replaces** (not merges) the subprocess env ([confirmed](https://code.claude.com/docs/en/agent-sdk/typescript)). So (a) you must spread `{...process.env}` to keep PATH/HTTPS_PROXY; (b) any stray `ANTHROPIC_API_KEY` in the sidecar env **silently outranks** the OAuth token (auth rank #3 > #5). Mitigation is *easier* in the explicit-env model: build the env map deliberately and never include `ANTHROPIC_API_KEY`. Token stays out of CLI args, same as today.
- **Fail-closed:** re-expressed (see boundary section above), not deleted. `SandboxSupervisor`-as-sole-health-writer + backoff stays; the guarded thing shifts from "remote argv" to "sidecar liveness via a direct signal."

### Deployment footguns specific to A (test on the actual k3s image)

- **musl vs glibc binary auto-discovery bug (#296, OPEN, unfixed):** the SDK can prefer the musl binary on a glibc system, failing with a *misleading* "native binary not found" ([confirmed](https://github.com/anthropics/claude-agent-sdk-typescript/issues/296)). Pin the correct optional dep for the OpenShell base image, or set `pathToClaudeCodeExecutable`. ❓ whether the base is Alpine(musl) or Debian(glibc) — **unknown, must verify on the real image**.
- **Per-query spawn overhead (~12s, issue #34):** see §5 — under-analyzed for the high-frequency short invocations and must be benched against today's baseline, not just flagged.
- **Billing change (both A and C):** from **June 15 2026**, subscription-plan Agent SDK / `claude -p` usage draws from a separate capped monthly "Agent SDK credit" pool ($20 Pro / $100 Max5x / $200 Max20x) ([confirmed](https://code.claude.com/docs/en/authentication)). This hits A and C **identically** (the CLI is in the same bucket), so it is **not a differentiator** — but it is a real operating-cost change to socialize regardless of the harness decision.

### `--max-budget-usd` is lateral, not an upgrade

The matrix uses budget cap to eliminate ACP (correct — ACP has no surface). But `maxBudgetUsd` is a **client-side estimate, not billing** ([confirmed](https://code.claude.com/docs/en/agent-sdk/cost-tracking)) — exactly the CLI's caveat today. For cron caps on spend-sensitive runs, A carries parity-with-a-known-weakness forward; it does **not** improve budget control. Don't let the matrix imply otherwise.

---

## 5. Migration cost & path

### What changes in the codebase

The migration is **concentrated**. RightClaw funnels every invocation through one builder:

- **`crates/bot/src/cc/invocation.rs`** — `ClaudeInvocation` + `into_args()` is the single chokepoint. Today it emits a `Vec<String>` of CLI args. The migration replaces `into_args()` with a serializer to a **sidecar request** (the SDK `query()` options: `model`, `maxTurns`, `maxBudgetUsd`, `mcpServers`, `outputFormat`, `resume`, `forkSession`, `persistSession`, `disallowedTools`, `systemPrompt`, `hooks`, `env`, plus whatever option restricts the tool set to empty — see the `--tools ""` gap). The builder's enforced invariants (bypass always on, mcp-config+strict together, fork-requires-resume, session-id priority fork>resume>new) map onto SDK options. `BASELINE_DISALLOWED_TOOLS` and the `disallow_*` composers are reused verbatim as the `disallowedTools` array. (Line numbers in the inventory are second-hand; verify on open.)
- **The ~9 callsites** (`telegram/worker.rs`, `reflection.rs`, `idle_compaction.rs`, `learning_probe_writer.rs`, `learning_curator.rs`, `learning_prefilter.rs`, `cron.rs`, `async_delivery.rs`, `background.rs`) keep their *shape* if you hold the builder's public API stable and swap only its internals + the spawn transport.
- **New component:** a **Node 18+ TS sidecar** (the SDK host) plus a Rust client that speaks to it over the host↔sidecar channel (see §4 — this is the genuinely new and *security-load-bearing* surface, not a throwaway detail).
- **`--system-prompt-file` has no SDK equivalent** — `systemPrompt` is a string ([partial](https://code.claude.com/docs/en/agent-sdk/hooks)). Read the composite prompt file in the sidecar and pass it as a string. **Prompt-caching concern:** verify a stable `systemPrompt` string caches as effectively as `--system-prompt-file`. ❓ **Must spike** — cost-critical (ARCHITECTURE.md flags prompt caching as load-bearing).

### Do the load-bearing features survive?

- **Forking — yes** (`resume + forkSession`), with the filesystem caveat unchanged (forked Bash edits hit the shared tree; checkpointing won't save Bash). Probe-writer and background-continuation forks port directly. ([confirmed](https://code.claude.com/docs/en/agent-sdk/sessions))
- **Caller-dictated session UUID — likely a parity GAP, not just "rework."** The CLI lets you *dictate* the UUID via `--session-id`, and RightClaw relies on this: session IDs are **persisted in the DB for recovery**, and the per-session mutex is keyed on `root_session_id` *before spawn* (a TOCTOU guard on the session JSONL). The evidence bundle (incl. the published `sdk.d.ts`) confirms `resume`/`resumeSessionAt`/`forkSession`/`model`/`env`/`maxBudgetUsd` but contains **no confirmation of a settable input session id** — `session_id` is documented only as *read back* from init/result. **If `query()` cannot accept a caller-chosen UUID:**
  1. the DB-persisted-UUID recovery model breaks (you must capture the SDK-assigned id post-spawn and persist *that*); and
  2. the mutex loses its key-before-spawn property — you'd spawn first, read the id, then lock, reintroducing the exact race the mutex exists to close.

  This is **answerable now from `sdk.d.ts`** and must be before committing. If unsupported, re-grade `--session-id` to ❌ and budget explicitly for a recovery/mutex redesign (e.g. a pre-spawn placeholder key promoted to the real id, or serializing per-*chat* rather than per-session). It does not kill A, but it materially raises migration cost.
- **Disable-all-tools (`--tools ""`) — unresolved parity gap.** Prefilter and idle-compaction disable *all* tools. `allowedTools:[]` does **not** do this (`allowedTools` auto-approves; it doesn't restrict — [confirmed](https://code.claude.com/docs/en/agent-sdk/permissions)). The SDK option that restricts the available tool set to empty (incl. MCP/harness) was not identified in the evidence. **Resolve from `sdk.d.ts` before committing**; two callsites depend on it.
- **Idle compaction — at risk.** No programmatic `/compact` trigger documented; only the `PreCompact` hook. Sending `/compact` as a prompt may work, but the "no schema, no MCP, exit-status-judged, **abortable-via-SIGKILL**" contract needs validation against SDK result subtypes **and** SDK abort semantics — does aborting `query()` SIGKILL the underlying binary's process group cleanly? Idle-compaction abortability is load-bearing (turn-start activity must cancel it). **Spike before committing.** ❓
- **Reflection — yes.** Resume + short turn + schema, expressible.
- **Learning pipeline — yes**, contingent on the disable-all-tools resolution for the prefilter, plus a structural opportunity: the non-foreground registration (temp `mcp-{id}.json` + `X-Right-Invocation` header) exists because the CLI loads MCP from a file. With in-process `createSdkMcpServer` you *could* collapse the temp-file dance — but that's a *refactor*, not required for parity. Keep the file-based aggregator path first; optimize later.

### Incremental rollout

1. **Spike (no production traffic), gating §6 items:** stand up the Node 18+ TS sidecar in one OpenShell sandbox; run a foreground turn with `systemPrompt` + `mcpServers` + `outputFormat:json_schema` + OAuth via `env`. Verify: prompt-cache hit-rate parity, structured-output subtype handling, NDJSON re-serialization, **musl/glibc resolves on the real image**, **the host↔sidecar channel + direct health signal**, and resolve the two `sdk.d.ts`-answerable gaps (session-UUID, disable-all-tools).
2. **Shadow the learning prefilter first** (no session, no fork; *but* needs the disable-all-tools answer) behind a per-agent flag; diff against the CLI path.
3. **Migrate the foreground worker** with PreToolUse hooks wired but initially pass-through (log-only), proving interception fires for MCP tools under `bypassPermissions`.
4. **Migrate forking paths** (probe-writer, background continuation) — riskiest; validate transcript/cache inheritance and the bypass+Bash interaction (§3.3).
5. **Migrate idle-compaction last**, only after the `/compact` trigger *and* abort/SIGKILL questions settle.
6. Keep `ClaudeInvocation`'s public API stable throughout so cutover is a builder-internals swap, not a callsite rewrite.

### Per-query spawn overhead — bench against the actual pattern

The ~12s spawn cost (issue #34, confirmed subprocess architecture) is **not** a generic "bench it." Reason about *which* invocations eat it: the learning pipeline fires **per foreground turn** — prefilter (Haiku, fresh, no session) + probe-writer fork + periodic curator, all short-lived. If each is a fresh `query()` = fresh subprocess spawn, you add spawn cost × (prefilter + probe-writer) to **every learning-eligible turn**, atop the foreground turn's own spawn. Today's CLI *also* spawns per-invocation, so the binary-spawn portion may be lateral — but the SDK adds a **JS-host init on top of** that spawn, and `startup()` pre-warming is unproven to survive across RightClaw's *separate* Rust callsites (prefilter and worker are different processes). **Establish the baseline comparison explicitly: "today N CLI spawns/turn" vs "SDK N subprocess spawns/turn + JS-host init."** If the JS host adds cost on top of the same binary spawn, it's a net regression for the high-frequency short invocations, and the learning pipeline is the worst case.

---

## 6. Open questions requiring resolution before committing

Items 1–2 are **answerable now from the `sdk.d.ts`/docs already in the evidence bundle** and should be resolved before any go decision — not deferred to a runtime spike. Items 3–9 require the spike. The recommendation is **contingent on items 1–4 resolving acceptably.**

1. **Caller-supplied session UUID (answer from `sdk.d.ts` now).** Does `query()` accept a UUID you choose, or only return one? *Decision-gating:* if no, re-grade `--session-id` to ❌ and budget for the DB-recovery + per-session-mutex redesign (the mutex's key-before-spawn TOCTOU guard breaks).
2. **Disable-all-tools equivalent (answer from `sdk.d.ts` now).** `allowedTools:[]` does NOT disable all tools. What option restricts the available tool set to empty (incl. MCP/harness), matching CLI `--tools ""`? *Decision-gating:* prefilter and idle-compaction depend on it.
3. **Prompt-cache parity for a `systemPrompt` string.** Does a stable string cache as well as `--system-prompt-file`? *Cost-critical.*
4. **Native binary on the actual OpenShell k3s base image.** musl or glibc? Does the glibc binary run cleanly, or must `pathToClaudeCodeExecutable` be pinned? *Must test on the real image — issue #296 is open.*
5. **Idle `/compact` — trigger AND abort.** Can the SDK *initiate* compaction (not just observe via `PreCompact`)? If only via a `/compact` prompt string, does it preserve "no schema, no MCP, exit-status-judged"? **And does aborting `query()` SIGKILL the underlying binary's process group cleanly** (turn-start activity must cancel idle compaction)? *Blocks idle-compaction migration.*
6. **Host↔sidecar trust boundary + direct health signal.** What ingress does the host use to reach the in-sandbox sidecar, and how is it encoded in OpenShell policy? What **direct** liveness signal does the sidecar expose (so `guard_no_sandboxed_host_exec` doesn't degrade to connectivity-as-readiness, which the project forbids)? *Needs design, not just a probe.*
7. **stream-json / NDJSON event-shape equivalence.** Does the SDK `SDKMessage` stream serialize to the same NDJSON the host logs expect? *Low risk; verify before declaring logging unaffected.*
8. **Spawn overhead for short non-foreground invocations.** Does `startup()` pre-warming bring prefilter/curator latency to acceptable levels across RightClaw's separate Rust callsites, or does the spawn + JS-host-init cost dominate vs today's CLI baseline? *Bench the learning pipeline specifically.* (Runtime note: run the sidecar under **Node 18+** — the documented floor; Bun in-sandbox `child_process.spawn` of the native binary is itself unverified and should not be assumed.)
9. **`canUseTool` is dead under bypass — confirm hooks suffice.** Since `bypassPermissions` is mandatory, smoke-test that a **PreToolUse hook** (not `canUseTool`) fires and can deny/rewrite for an `mcp__right__*` tool. *Low risk; closes the last interception inference.*

**Files to touch first:** `crates/bot/src/cc/invocation.rs` — the `ClaudeInvocation` builder and `into_args()` is the single chokepoint; `BASELINE_DISALLOWED_TOOLS` and the `disallow_*` composers are reused as the `disallowedTools` array. The ~9 callsites in §5 change only if the builder's public API changes — keep it stable.

---

### Bottom line for the decision-maker

A is the right direction — the only (near-)superset of today's control surface that keeps the loop in the box — but the go/no-go is **gated on the spike**, and two of the gates (caller-dictated session UUID, disable-all-tools) are answerable from the SDK type file *before* writing any sidecar code. Resolve those first: if the session UUID can't be pre-assigned, the migration cost rises (recovery + mutex redesign) but A still wins; if it can, A is a clean superset with operational, not architectural, costs. Run the sidecar on **Node 18+**, treat `bypassPermissions` as mandatory (so interception is hooks-only), and design the **host↔sidecar ingress + direct health signal** before claiming the security model is preserved.

---
---

# Round 2 — without the Bun constraint; ACP vs Agent SDK vs raw Rust control protocol

> **Verification caveat for this round:** the adversarial verify phase largely failed — 9 of 10 high-stakes fact-checkers did not return structured output, so only **one** high-stakes verdict was independently re-grounded: `option-d-control-protocol-churn` (= the native control protocol is internal/unversioned), which is the round's decisive argument. The rest rest on a single research pass plus the synthesis critique loop (which did catch several mis-citations and is reflected in the §7.6 "re-ground these quotes" item). Treat round-2 confidence as softer than round 1, and execute §7.6 before this drives code.

## 1. Updated recommendation

**Recommendation: A — TS Agent SDK sidecar on Node 20 LTS. Confidence: moderate-high (~75%), conditional on the §7 in-sandbox hook spike.**

Round 1 stands. Option D (pure-Rust client of the native stdio control protocol) is **capability-complete but rejected on stability grounds**, and that single factor is decisive. The recommendation is **conditional**: both A and D deliver the round's actual goal (programmatic interception) only if `PreToolUse` `updatedInput` rewrite fires under `--dangerously-skip-permissions` against the sandbox CLI build — and that has **not** been verified for either option (see §7, item 1). Until that spike passes, no option delivers "more control."

**The deciding factor between A and D:** the outer SDK↔CLI control protocol that carries `can_use_tool` and `hook_callback` is **internal, unspecified, and has no version-negotiation handshake**. The verdict on `option-d-control-protocol-churn` is `confirmed`:

- No Anthropic wire spec exists; every public description is reverse-engineered. The Roasbeef Go SDK doc states verbatim: *"Most of this is undocumented in official sources and was discovered through implementation and testing"* ([cli-protocol.md](https://raw.githubusercontent.com/Roasbeef/claude-agent-sdk-go/main/docs/cli-protocol.md)).
- The control channel has **no version field to pin against**. The `protocolVersion "2024-11-05"` visible in traces belongs to the *inner* MCP JSON-RPC handshake (wrapped in `mcp_message`), not the outer control channel. The `initialize` control subtype registers hooks + SDK MCP servers and carries no protocol-version field.
- The SDK CHANGELOG shows the control surface mutating version-for-version with the CLI: `0.3.161` "ControlResponse gains an optional pending_permission_requests field"; `0.2.83` "Added seed_read_state control subtype"; `0.2.76` "Added cancel_async_message control subtype"; plus 100+ "parity with Claude Code v2.1.X" entries ([CHANGELOG](https://github.com/anthropics/claude-agent-sdk-typescript/blob/main/CHANGELOG.md)).

A hand-rolled Rust client cannot detect that the binary it spawned speaks a drifted protocol; it can only fail at runtime on an unexpected subtype. The churn is predominantly *additive* (new optional fields, new subtypes) rather than frequent breaking rewrites — but for an unversioned client, an unrecognized new subtype is itself an unhandled-message failure. This is exactly the failure mode RightClaw must avoid: per AGENTS.md, OpenShell is alpha and the platform mandate is self-healing and resilient. The TS SDK absorbs that churn because Anthropic re-tests its control client against each CLI build and ships them together.

**The conditional, stated explicitly:** D *would* dominate A on architecture (zero JS runtime in the sandbox, full Rust control, one language) **if and only if** the control protocol were public, specified, and version-negotiable. It exposes `can_use_tool` + hooks to the client (capability gate passes — see §2), so it fails only the *stability* gate. If Anthropic ever publishes and versions this protocol, re-open D immediately.

## 2. Option D verdict — capability yes, stability no

**Can pure Rust get tool interception + hooks via the control protocol? Yes, mechanically — and this is now grounded in real source.**

- The binary dispatches `can_use_tool` and `hook_callback` to the client *as incoming control requests*. The non-JS Roasbeef Go SDK handles exactly these in `handleControlRequest()`: `case "can_use_tool":` and `case "hook_callback":` ([protocol.go](https://raw.githubusercontent.com/Roasbeef/claude-agent-sdk-go/main/protocol.go)). This is the load-bearing proof that a non-JS (hence Rust) client can answer interception requests — the binary drives the dispatch, the client need only respond.
- A `can_use_tool` response may carry `updatedInput` (input rewrite) — this is the SDK's documented `canUseTool` contract; **the precise wire shape should be re-grounded to `sdk.mjs`/`sdk.d.ts` from the npm tarball, not the npm README** (see §7, item 6; the prior draft mis-cited this to the README as a verbatim quote — corrected to "paraphrase, verify against SDK source").
- Hooks survive bypass mode: *"Bypass permissions mode hooks still execute and can block operations if needed"* ([Agent SDK permissions docs](https://code.claude.com/docs/en/agent-sdk/permissions)). Note this confirms hooks *run* under bypass; it does **not** confirm `can_use_tool` is *suppressed* under bypass.

**Critical interception caveat — DOWNGRADED to UNVERIFIED.** The round-1 framing "under skip-permissions `can_use_tool` is not emitted; rewrite is hooks-only" is **not supported by its cited source.** `protocol.go` contains no `bypassPermissions`/`skip-permissions` logic and no guard suppressing `can_use_tool`. The permissions doc confirms only that hooks run under bypass, not that `can_use_tool` is suppressed. Honest status:

- **Unverified (a):** that `can_use_tool` is suppressed under `--dangerously-skip-permissions`. No primary source shows this guard.
- **Unverified (b):** that `PreToolUse` → `permissionDecision: allow` + `updatedInput` actually rewrites tool input under bypass against the CLI build RightClaw ships.

Both must be tested end-to-end in-sandbox (§7). This caveat applies equally to A and D — neither has demonstrated input rewrite under bypass. The §5 parity table reflects this as **UNVERIFIED**, not "supported."

**Stability risk: the disqualifier.** See §1 and the confirmed verdict. Reverse-engineered, no spec, no handshake, additive-but-unhandled drift, control surface shipped version-locked to every CLI build. This is recurring maintenance tax on the most critical path (every CC turn) — the reason to reject D in favor of A.

**What Rust would have to implement and keep re-implementing:** spawn with `--input-format stream-json --output-format stream-json`; the `control_request`/`control_response` envelope; the `initialize` subtype registering hooks + SDK MCP servers; the `hook_callback` request/response cycle for `PreToolUse`; line-delimited JSON framing/buffering; and a contract test pinned to a specific CLI version that must be re-run and likely re-fixed on every CLI bump.

## 3. Does ACP survive re-examination? — Yes, rejection confirmed; round-1 *reasoning* partly corrected

**Verdict: ACP stays rejected, dominated by both A and D — but on corrected grounds.** Two round-1 sub-claims were wrong and are corrected; the rejection is re-argued on the *right* axis (structured-output round-trip and hook surface), not on "sidecar vs no sidecar."

Corrections in ACP's favor (steelman):
- **Options DO pass in** via `_meta.claudeCode.options` — the adapter merges them, stripping only ~6 fields (`cwd, includePartialMessages, allowDangerouslySkipPermissions, permissionMode, canUseTool, executable`), so `outputFormat`, `maxTurns`, `forkSession`, `model`, `systemPrompt` flow through ([acp-agent.ts](https://raw.githubusercontent.com/agentclientprotocol/claude-agent-acp/main/src/acp-agent.ts)). Round-1's "no surface for these" was wrong.
- **Forking IS available** via `unstable_forkSession` (resume + `forkSession: true`). Round-1's "no session forking" was wrong.

The disqualifiers that survive:
- **Tool-input rewrite is impossible through the standard protocol+adapter.** `RequestPermissionResponse` carries only a selected option ID, not modified tool input ([ACP schema](https://raw.githubusercontent.com/agentclientprotocol/agent-client-protocol/main/schema/schema.json)); the adapter literally returns `{ behavior: "allow", updatedInput: toolInput }` — the *original* input, unchanged. Under bypass this surface is skipped entirely, and ACP exposes no hook surface to the client. Net: **less interception than A or D.**
- **Structured output cannot round-trip — but this is an *adapter* gap, not a protocol limit.** `outputFormat` passes *in*, but the adapter's prompt response is `return { stopReason, usage: sessionUsage(session) }` — no `structured_output` field. ACP responses *do* have `_meta`, so `structured_output` *could* be carried there, but no one has wired it. So the precise rejection is: "the current adapter drops structured output; ACP's `_meta` could carry it but nothing implements that." RightClaw depends on `--json-schema` structured output every turn, and recovering it means patching the Node adapter.
- **No production Rust ACP client** drives Claude Code; only the inverse server `claude-code-acp-rs` v0.1.22 exists ([docs.rs](https://docs.rs/crate/claude-code-acp-rs/latest)). RightClaw would build a bespoke client.

**Why ACP loses on the corrected axis.** Both A and ACP-recovery require shipping/patching a Node artifact, so "ACP buys a Node sidecar" is *not* the differentiator (A ships one too). ACP loses because: (1) input rewrite is impossible without forking the adapter, (2) structured-output round-trip is unimplemented and would require adapter changes, and (3) there is no maintained Rust ACP client — so the path is "patch a third-party Node adapter to recover capabilities the SDK already exposes directly." Patching/using the SDK (option A) is the more direct target for the same artifact class. **Dominated. Rejected.**

## 4. Runtime decision (A) — Node 20 LTS

**Choose Node 20 LTS.** Node 18 is the SDK's documented floor but is past LTS maintenance; Node 20 is the current LTS with full node-compat and first-class `process.report.getReport`, which the SDK's musl-binary picker relies on. (The choice is Node 20 *over Node 18* for LTS support — not because the SDK requires 20.)

- SDK `engines` floor is Node 18; runtime auto-detection picks `bun` only if `process.versions.bun` is set, else `node`. Deno is never auto-defaulted. (Re-ground the exact engines/auto-detect strings to `sdk.mjs`/`package.json` in the npm tarball — the prior draft cited a paraphrase to the README.)
- The SDK **bundles its own native CLI** as `optionalDependencies` (prefers musl), resolves via `createRequire`, and spawns it directly — the host runtime is **not** in the spawn argv (it is prepended only for `.js`/`.mjs`/`.ts` executable paths). So the JS runtime only hosts the thin control loop; it does not wrap the binary. **This claim must be re-grounded to `sdk.mjs` source, not the npm README** (§7, item 6).
- **Bun** (second choice): Anthropic-owned, fast, single-binary compile via `extractFromBunfs`. Attractive, but its in-sandbox `child_process.spawn` of the native binary plus the `process.report` musl detector are **UNVERIFIED** at runtime. Not worth the risk on the critical path when Node is proven.
- **Deno** (rejected): `child_process` is *"Partially supported"* and `process.report` is undocumented ([Deno node APIs](https://docs.deno.com/runtime/reference/node_apis/)), so the musl detector may misfire unless `pathToClaudeCodeExecutable` is set explicitly. Avoidable fragility.

**Single-binary compile:** feasible in principle (Bun `--compile` or `node --experimental-sea`), but **UNKNOWN** whether the bundled musl-CLI extraction survives SEA packaging inside the OpenShell filesystem/landlock. Treat as a spike, not a requirement — `node sidecar.mjs` + uploaded `node_modules` is the safe baseline.

## 5. Feature parity for the chosen option (A, stream-json control mode)

| Capability | Status under A (TS SDK sidecar) | Evidence |
|---|---|---|
| `--session-id` / `--resume` | Supported (already used by RightClaw today) | round-1 + CLI ref |
| Session forking | Supported (`forkSession: true`) | SDK Options / acp-agent.ts confirms field exists |
| `--json-schema` structured output | Supported; **proven in RightClaw today** with the native binary | already in production |
| `--max-budget-usd` budget cap | Supported (general CLI flag) | [CLI ref](https://code.claude.com/docs/en/cli-reference) |
| `--max-turns` | Supported (general CLI flag) | CLI ref |
| `--model`, `--mcp-config`, `--system-prompt-file`, `--disallowedTools` | Supported (general flags) | CLI ref |
| Hooks (`PreToolUse` etc.) fire under bypassPermissions | Supported (hooks *run* under bypass) | [permissions docs](https://code.claude.com/docs/en/agent-sdk/permissions) |
| **Tool-input rewrite (`PreToolUse` → `updatedInput`) under `--dangerously-skip-permissions`** | **UNVERIFIED — gating spike (§7.1).** No primary source confirms it fires under bypass; the round's whole goal depends on it. | cited source (protocol.go) shows no bypass guard; must test in-sandbox |
| `can_use_tool` suppressed under bypass | **UNVERIFIED** — no source shows the guard | — |

All of RightClaw's current invariant flags (`--dangerously-skip-permissions`, `--mcp-config`/`--strict-mcp-config`, `--output-format`, `--json-schema`) are general CLI flags and survive stream-json control mode (these are proven today). The *new* capability A is meant to unlock is **programmatic hooks with input rewrite** — and that specific behavior under bypass is the unverified gate, not a settled "supported."

## 6. Migration delta vs round-1 plan

Round 1 already chose A, so the **plan is structurally unchanged** — round 2 confirms it against option D and the corrected ACP picture. Net delta: positive evidence that D and ACP are inferior, plus **one new must-resolve risk** (the two-binaries question below) and one gating spike (§7.1).

Integration points under A:
- **`crates/bot/src/cc/invocation.rs` remains the single chokepoint.** `ClaudeInvocation` keeps enforcing invariant flags. For hook-bearing turns it targets the **sidecar** (which spawns the bundled binary) rather than the binary directly, adding `--input-format stream-json` + the sidecar's hook config. `guard_no_sandboxed_host_exec` still applies — the sidecar runs *inside* the sandbox, so the host-exec fail-closed invariant is unaffected.
- **New artifact:** a Node 20 sidecar (`@anthropic-ai/claude-agent-sdk`) uploaded into the sandbox, registered as a `Regenerated(BotRestart)` codegen output (per ARCHITECTURE.md codegen categories) so already-deployed agents adopt it via `right restart` with no sandbox recreation.
- **Two-binaries problem — MUST be resolved before implementation.** The SDK bundles its *own* musl `claude` binary; RightClaw already vendors a `claude` binary in the sandbox. If the sidecar runs the SDK's bundled binary, RightClaw loses control of the CLI version actually executing every turn; if it sets `pathToClaudeCodeExecutable` at RightClaw's vendored binary, it **reintroduces the exact SDK↔CLI version-skew risk D was rejected for** — the SDK's control client is tested against the SDK's bundled CLI, not RightClaw's vendored one. Decision required: pin the SDK↔CLI pair (preferably let the SDK use its bundled binary, and have RightClaw vendor *that* SDK release as the source of truth for the in-sandbox CLI), and add a contract test on the actual pair. A's entire resilience advantage over D is contingent on this pairing being version-locked and not drifting independently.
- **Hook execution locus:** TS-resident hooks vs. dispatching decisions back to the host control plane — **design decision deferred to spike** (§7.4). TS-resident is the simplest first cut; host round-trips add an ingress boundary.
- **No `.mcp.json` / no host-fallback changes.** Aggregator, internal Unix socket, and sandbox MCP routing are untouched.

Because A keeps the binary as the actual executor, today's proven `--json-schema` + stream-json behavior carries over — we are *adding* a control client, not replacing the execution engine.

## 7. Open questions / spike items

1. **GATING PRECONDITION (not merely open): do `PreToolUse` hooks reliably fire AND rewrite `updatedInput` under `--dangerously-skip-permissions` against the *current* CLI build inside OpenShell?** Until this is tested end-to-end in-sandbox, **neither A nor D delivers the round's stated goal**, and the recommendation is provisional. The §5 table marks this UNVERIFIED. This is make-or-break.
2. **Verify `can_use_tool` suppression under bypass.** The round-1 "hooks-only under bypass" caveat is currently *unverified* — its cited source (`protocol.go`) has no such guard. Either find primary source, or confirm empirically in-sandbox.
3. **Pin a CLI version + contract test for the SDK↔CLI pair.** Applies to A (rides the same control channel internally) and resolves the two-binaries risk in §6. A parity bump must not silently break hooks.
4. **Single-binary sidecar packaging in OpenShell.** Does `node --experimental-sea` (or Bun `--compile`) preserve the SDK's bundled-musl-CLI extraction under sandbox filesystem/landlock? Baseline fallback: `node sidecar.mjs` + uploaded `node_modules`. **UNKNOWN.**
5. **Hook execution locus:** TS-resident vs. host round-trip over a new ingress. Decide on what the interception logic needs (host DB/state → round-trip + new boundary; pure input validation → keep in TS). **Design-open.**
6. **Re-ground three citations to primary source before this report drives implementation** (research-rules compliance): (a) D3 `can_use_tool` + `updatedInput` wire shape → `sdk.mjs`/`sdk.d.ts` from the npm tarball, with the dispatch-direction proof quoting `protocol.go`'s `case "can_use_tool":`/`case "hook_callback":`; (b) runtime claims C1/C2 (engines floor, bundled-binary spawn) → `sdk.mjs`/`package.json`, not the npm README; (c) drop or replace the fabricated `cli-protocol.md` "It does not require a Node sidecar" quote — that file actually says "Most of this is undocumented in official sources." Every paraphrase presented as a verbatim quote must be relabeled "(paraphrase)" or replaced with real bytes.
7. **Maturity of `claude-agent-sdk-rs` / `claude-codes`** — not load-bearing for A. If either becomes a *maintained, versioned* Rust client tracking the CLI, it changes the D calculus. Currently **UNKNOWN/unverified.**
8. **Confirm the binary accepts `--input-format stream-json`** in the exact sandbox CLI build (RightClaw proves `--output-format` today, not necessarily bidirectional input mode).

---

**Round-2 bottom line:** Stay on A — TS Agent SDK sidecar, Node 20 LTS, sidecar inside the sandbox, `crates/bot/src/cc/invocation.rs` as the chokepoint — **conditional on the §7.1 in-sandbox hook-rewrite spike passing**, since that behavior is unverified for both A and D and is the round's actual goal. D is the architecturally cleaner, capability-complete answer; it loses only on the one axis RightClaw cannot compromise on the critical path: the control protocol is internal, unspecified, and unversioned (verdict `confirmed`). Two corrections to act on before implementation: (1) resolve the two-binaries / version-pinning question in §6, which is the sole basis for A's resilience edge over D, and (2) re-ground the three mis-cited capability quotes per §7.6. Revisit D the day Anthropic publishes and versions the wire format. ACP remains dominated — on structured-output round-trip and hook surface, not on "sidecar vs no sidecar."

---
---

# Round 3 — Claude Agent SDK (chosen) vs Pi ecosystem vs opencode (model-agnostic / self-host)

> Motivation: the user wants to use different models including self-hosted, doesn't care about migration cost, but requires **every** load-bearing capability to remain available (skills, forks, subagents, structured output, MCP, …). This round compares the chosen baseline against two model-agnostic open-source engines that would **replace Claude Code entirely**: **Pi** (`@earendil-works/pi` + `oh-my-pi` + `pi-acp`) and **opencode** (`sst/opencode`). Verify phase: 14/14 high-stakes claims independently re-grounded against primary source.

## 1. TL;DR Recommendation

**Stay on the baseline (Claude Agent SDK sidecar). Do not switch to Pi. opencode is the only credible challenger and the only one worth a time-boxed spike — but it is not a switch-now recommendation.**

**Confidence: high** that Pi is disqualified; **medium-high** that opencode is "not yet."

**Single deciding factor:** the user's main draw — *reliable* multi-model/self-host — is not actually delivered by either challenger. opencode meets every hard-gate capability on paper, but every load-bearing differentiator RightClaw depends on (structured output, prompt caching, skill-following, tool-call reliability) is **provider-dependent and tool-call-emulated, not natively enforced**, on exactly the non-Claude/self-hosted models the user wants. You would trade Anthropic-guaranteed mechanics for "works if the model is good enough," while keeping all the integration cost. The engine supporting model X is not the same as RightClaw's Claude-tuned prompts/schemas working on model X — and the evidence says they degrade, not transfer.

**A correction the spike must respect (this changes the parity read):** opencode's structured output is present in the **OpenAPI spec on the v1 `/session/{id}/message` route** (verdict c5) — but the **generated SDK `types.gen.ts` omits the `OutputFormat` types entirely**, and the v2 prompt route (`POST /api/session/{id}/prompt`) omits `format` from its body too (verdict c5). So the typed SDK RightClaw would naturally drive **cannot pass `format` today without raw-body HTTP calls**. Two load-bearing mechanics — structured output (reachable only via raw-body HTTP) and learned-skill usage receipts (no documented signal, see §2) — are "buildable but unverified at source." Under the user's hard requirement ("every load-bearing capability remains available"), these are best read as **not-yet-met pending a spike**, not as quiet ⚠️-passes.

The honest framing: this is a **bet on degradation tolerance plus two unverified-at-source builds**, not a clean feature swap. Run the opencode spike (§6, §7) only when the user has a *specific* self-hosted model that must be supported and accepts measured structured-output degradation. Until then, the baseline meets 100% of load-bearing capabilities natively and the switch buys aspiration, not capability.

## 2. Hard-Gate Parity Matrix

Load-bearing rows from the inventory. Cell = status + killer caveat. **DISQUALIFYING gaps in bold.**

| Capability | Baseline (Claude Agent SDK) | Pi (vanilla @earendil-works/pi) | opencode (sst) |
|---|---|---|---|
| **MCP tool layer** (entire RightClaw tool surface) | ✅ native | **❌ UNSUPPORTED — no built-in MCP at all** (verdict C13: zero `mcp`/`modelcontextprotocol` refs in `coding-agent/src`). Only via `oh-my-pi` — but its MCP *client* is unverified at source. | ✅ local stdio + remote HTTP w/ Bearer, runtime-addable via `mcp.add` (verdict c9). RightClaw aggregator ports as a `type:"remote"` entry. |
| **Structured output (json-schema)** | ✅ native constrained decoding, all models | **❌ UNSUPPORTED natively** — only forced tool calls (verdict C13). | ⚠️ **present-but-SDK-unreachable + emulated.** Spec has `format` on v1 `/session/{id}/message` (c5), implemented as synthetic `StructuredOutput` tool + `toolChoice:"required"` (c4) — NOT constrained decoding. Typed SDK omits `OutputFormat`; v2 route omits `format` → reachable only via **raw-body HTTP**. |
| **Skills (ClawHub/OpenClaw SKILL.md)** | ✅ native | ⚠️ skills exist; SKILL.md drop-in unverified | ⚠️ **docs-only, unverified at source.** Discovery from `.claude/skills` claimed by docs, no verdict. Source-confirmed negative: frontmatter parser reads only `name/description/slash` — **`allowed-tools` silently ignored** → per-skill tool restriction not enforced. |
| **Skill learning pipeline** (prefilter→probe-writer fork→curator) | ✅ on `claude -p` fork/`--json-schema`/`--max-turns` | ❌ full rebuild + missing MCP/structured-output substrate | ⚠️ **doubly-dependent rebuild.** Re-expressible on fork + structured-output + tool-restrict, but the `used_skill_receipts` ledger needs (a) structured-output reliability and (b) a skill-invocation signal opencode is **not documented to emit**. Signal gap = verification gap, not cost. |
| **Session forking** | ✅ `--fork-session` | ⚠️ fork/clone documented | ✅ `POST /session/{id}/fork` at a messageID, DB-backed (verdict c10). |
| **Subagents** | ✅ native Agent tool | **❌ UNSUPPORTED by design** — author explicitly omits a sub-agent tool. Only in `oh-my-pi`. | ✅ `task` tool, child sessions, `SubtaskPartInput` (verdict c11). |
| **Session resume / id / persistence** | ✅ `--resume`/`--session-id` | ⚠️ "resume" is retry, not checkpoint (C7) | ✅ durable DB-backed sessions (c10). ⚠️ no `resume` flag — implicit via re-prompt; open issue suggests history loss on restart — spike. |
| **Prompt caching** | ✅ native Anthropic 5-min ephemeral | ⚠️ generic provider caching; Anthropic `cache_control` **not** found (C7) | ⚠️ `applyCaching` **gated to Anthropic-family models, excludes `@ai-sdk/gateway`** (c6). On self-host/vLLM markers inert, only auto prefix caching. |
| **Composite system prompt** | ✅ `--system-prompt-file` | ⚠️ partial | ✅ per-prompt `system` field (c15). ⚠️ replace-vs-augment unverified — spike. |
| **Budget caps (`--max-budget-usd`)** | ✅ | ❌ no USD/turn cap (C13) | ❌ only agent-level `maxSteps`; RightClaw must enforce host-side (abort via `POST /session/{id}/abort`). |
| **Tool restrict/disable** | ✅ `--allowedTools`/`--disallowedTools` | ✅ via ExtensionAPI (build-it-yourself) | ✅ per-prompt `tools:{name:bool}` map. ⚠️ `--tools ''` equivalence — spike. |
| **NDJSON stream logging** | ✅ `--output-format stream-json` | ❌ not found in vanilla (C7) | ✅ SSE `GET /event`. ⚠️ per-turn token/cost shape mapping unverified — spike. |
| **Per-agent auth** | ✅ `CLAUDE_CODE_OAUTH_TOKEN` | ⚠️ `getApiKey` dynamic callback (C7) | ⚠️ global-per-install + per-provider; per-agent isolation likely needs one server per sandbox (matches RightClaw model; unverified). |
| **Idle compaction** *(important)* | ✅ `/compact` resume turn | ⚠️ automatic compaction | ✅ `summarize` API + auto-compaction. ⚠️ out-of-band trigger w/o deliverable turn — spike. |
| **Reflection primitive** *(important)* | ✅ `--resume` notice turn | ⚠️ rebuildable | ✅ rebuildable on resume + system-inject + abort. |

**Verdict from the matrix:**
- **Pi: DISQUALIFIED.** Three hard gates unmet by design — MCP, subagents, native structured output — and RightClaw's *entire* tool layer is MCP. The only escape is `oh-my-pi`, but its **MCP *client* is not source-verified** (verdict `mcp`: the cited file is *"a 74-line MCP config schema, not a client … repo-wide search finds no MCP client code"*). Single-maintainer MIT fork, highest bus-factor on the table. Not a serious candidate.
- **opencode: PASSES the binary gates** (MCP ✅, subagents ✅, fork ✅) **but two load-bearing mechanics are "present-but-unverified-at-source"**: structured output reachable only via raw-body HTTP, and learned-skill usage receipts have no documented signal. Plus three quality caveats that hit exactly the user's draw: structured output is emulated, caching is Claude-gated, skills lose `allowed-tools`.
- **Baseline: 100% native** with Anthropic-guaranteed mechanics. Its one *economic* risk is in §5.

## 3. The Multi-Model / Self-Host Reality (the user's main draw)

This is where the case for switching collapses under its own premise. **Engines supporting model X ≠ RightClaw's Claude-tuned stack working on model X.**

| What | Claude baseline | opencode on non-Claude / self-hosted | Honest verdict |
|---|---|---|---|
| **Structured output enforcement** | Native constrained decoding — schema *guaranteed* | Synthetic tool + `toolChoice:"required"` + retry (c4). vLLM *can* enforce via `guided_json` but **opencode does not wire it**; typed SDK can't even pass `format` (c5) | **Aspiration, not shipped through the SDK.** RightClaw passes `--json-schema` every turn; on weak open models this becomes a soft request with retries. Biggest reliability regression. |
| **Tool-calling reliability** | Tuned for Claude | Provider-dependent; not guaranteed off-Claude | Varies by model, **unmeasured** off-Claude. Every MCP call + the structured-output emulation ride on tool-calling. *(An earlier BFCL citation was dropped — the quoted line actually showed an open model leading, so it did not substantiate "weaker off-Claude.")* |
| **Prompt caching** | Native 5-min ephemeral; drives the whole "don't re-read identity per turn" economy | Auto cache_control **gated to Anthropic-family** (c6); on vLLM markers inert, only prefix caching | **Caching economics do not transfer to self-host.** Composite-prompt caching assumption breaks; cost model changes. |
| **Skill-following** | Claude-tuned skills written for Claude | SKILL.md ports as plain text; `allowed-tools` ignored; usage-receipt signal undocumented | Skills load (docs-claimed) but enforcement weakens; following quality provider-dependent. |
| **Anthropic subscription economics** | Multi-agent-on-one-subscription via OAuth | **Gone** — Anthropic blocked 3rd-party Claude subscription OAuth 2026-01-09 | **Also threatens the baseline** — see §5. Native `claude` binary *likely* retains it, but **not confirmed**. |

**Bottom line:** opencode genuinely lets you *point at* any OpenAI-compatible/self-hosted endpoint (c7 — real and shipped). But the mechanics RightClaw leans on hardest all degrade from *guaranteed* to *best-effort* the moment you leave Claude, and that degradation is **unmeasured**. The engine delivers connectivity, not reliability.

## 4. Programmatic Control

| Dimension | Baseline | Pi | opencode |
|---|---|---|---|
| **Driving model** | CLI argv via `ClaudeInvocation` | stdio `rpc` mode (`pi --mode rpc`), MVP-grade, internal | **HTTP/JSON server** (`opencode serve`, OpenAPI 3.1) + typed SDK (c1,c2) — best surface of the three |
| **Tool-call interception** | via MCP aggregator (RightClaw owns it) | `beforeToolCall`/`afterToolCall` (C7) | **First-class hooks**: `tool.execute.before/after`, `permission.ask`, `tool.definition`, `chat.params/headers`, system/messages transforms (c3 verbatim). Richest layer. |
| **Subagents** | native Agent tool | none (vanilla) | `task` tool + `SubtaskPartInput` (c11) |
| **SDK type safety** | N/A (argv) | TS types | `@opencode-ai/sdk` typed (c2). ⚠️ **types lag spec** — `OutputFormat` omitted; target v1 `/session/{id}/message`, raw-body for `format` (c5) |

opencode wins programmatic control decisively — its hook system is more powerful than the baseline exposes natively. Two precision caveats: (1) v1 client has no `permission.respond` — use `POST /session/{id}/permissions/{permissionID}`; clean namespace is v2-only (c2); (2) server **defaults to unsecured** — `OPENCODE_SERVER_PASSWORD` is mandatory (c1).

## 5. Ecosystem / Maturity / Risk

| | Baseline | Pi / oh-my-pi | opencode |
|---|---|---|---|
| **Backing** | **Anthropic** — institutional | earendil-works (M. Zechner) + oh-my-pi single maintainer | anomalyco (ex-SST), community-led |
| **License** | proprietary | MIT | MIT |
| **Bus factor** | low | **high** — fork-dependent for MCP/subagents; oh-my-pi MCP client unverified | moderate — two-person core, no SLA |
| **Cadence** | Anthropic train | active but small | **very high** — multiple releases/week |
| **Adoption** | first-party | "powers OpenClaw" — **unverified** (C7 found zero OpenClaw mentions in the pi repo) | ~170k stars |
| **API stability** | stable | rpc internal/unversioned | v1 stable; **mid-flight Effect-based v2 rewrite** with structured-output/MCP/skills UNCHECKED (c12). Pin to v1. |
| **Baseline economic risk** ⚠️ | Anthropic **blocked third-party Claude subscription OAuth on 2026-01-09**. Whether the native `claude` binary retains subscription economics post-block is *implied, not confirmed* (§7-#14). | n/a | n/a |

opencode's velocity is double-edged: high cadence + active v2 rewrite means the integration target moves — pin to v1 and accept churn. Pi's risk is categorically worse. The baseline is the lowest-risk **capability** option, but its **economic** advantage is unverified post-2026-01-09.

## 6. Migration Architecture

### Challenger winner: opencode (Pi is not architecturally viable)

- **`invocation.rs` chokepoint** becomes an **HTTP driver**: `POST /session/{id}/message` with `{model, agent, system, tools, format, parts}` against an in-sandbox `opencode serve`. **`format` is not in the typed SDK → raw-body HTTP for every json-schema turn** (c5). The one-builder contract is preserved; only the wire format changes.
- **OpenShell sandbox runtime:** replaces SSH-exec with a **host→sandbox HTTP ingress on `:4096`**. Needs: a new OpenShell policy endpoint; `OPENCODE_SERVER_PASSWORD` (mandatory — defaults unsecured); a direct health signal for `SandboxSupervisor`; **one server per agent per sandbox**. Loop+tools stay in the box. `guard_no_sandboxed_host_exec` generalizes to "refuse to drive a host-side opencode."
- **Auth/billing shift:** `CLAUDE_CODE_OAUTH_TOKEN` → provider API keys / gateway base URLs as env-templated headers. **Composes well** with RightClaw's gateway placeholder substitution. Cost: Claude subscription economics abandoned.
- **MCP reuse:** RightClaw's aggregator ports nearly unchanged as a `type:"remote"` MCP entry with per-agent Bearer (c9). Cleanest part. Hindsight memory stays.
- **Rebuilt machinery:** learning pipeline (fork + raw-body `format` + per-prompt `tools` whitelist — receipt risk: needs reliable structured output *and* an undocumented skill-invocation signal); composite prompt → per-prompt `system`; idle compaction → `summarize`; budget caps → host-side abort; NDJSON → SSE re-derivation.

### Runner-up: Pi — **not recommended.** Would require `oh-my-pi` for MCP+subagents, but its MCP client is unverified at source, it ships a colliding "Hindsight" memory, over an MVP-grade unversioned stdio rpc. Single-maintainer fork dependency for *unverified* load-bearing capability is unacceptable for a security-first closed-box platform.

## 7. Open Questions / Spike Items (opencode)

Decisive gates (1–3) must pass before any commitment:
1. **Structured-output reliability off-Claude** — measure RightClaw's Claude-tuned schemas via opencode's `StructuredOutput` tool against ≥2 self-hosted models + Claude; real failure/retry rate? Decides everything.
2. **Structured output reachable at all** — confirm `format` passable via **raw-body HTTP** on v1 `/session/{id}/message` (typed SDK omits it — c5). If only SDK path is viable, opencode fails the hard gate.
3. **Learned-skill usage signal** — does the `skill` tool surface a per-invocation signal mappable to `used_skill_receipts`? Without it the `skill_lifecycle`/`skill_spend` `kind='usage'` ledger is dead.
4. Skills drop-in (docs-only today) — verify `.claude/skills` actually loads `rightx-*`; only confirmed fact is the negative `allowed-tools` drop.
5. Self-host enforcement — can opencode pass vLLM `guided_json` without forking the engine?
6. Prompt caching on self-host — confirm markers inert on vLLM (c6).
7. System-prompt replace-vs-augment (c15).
8. Session persistence across server restarts (open issue; c10).
9. Per-turn SSE event shape → NDJSON + `usage_events`/`skill_spend`.
10. Per-invocation tool restriction incl. prefilter `--tools ''`.
11. Idle compaction out-of-band (no deliverable turn).
12. Per-agent auth isolation via one-server-per-sandbox + `:4096` basic-auth (c1).
13. v1↔v2 stability across weekly releases (c12).
14. **UNKNOWN — Baseline OAuth retention.** Does the native `claude` binary retain Claude subscription OAuth post-2026-01-09? Implied, not confirmed — affects the *recommended* option's own economics.

### Executive summary

Pi is disqualified — vanilla Pi lacks MCP, subagents, and native structured output (RightClaw's entire tool layer is MCP), and the fixes live only in a single-maintainer fork whose MCP client is unverified at source. opencode clears the binary hard gates and offers the best programmatic surface, but two load-bearing mechanics are present-but-unverified-at-source (structured output reachable only via raw-body HTTP; learned-skill usage receipts undocumented), and the multi-model advantages the user wants degrade from Anthropic-*guaranteed* to provider-dependent *best-effort* off Claude, unmeasured. Because the stated draw is *reliable* self-host and the hard requirement is that **every** load-bearing capability remains available, the responsible call is to **stay on the Claude Agent SDK baseline now** — while acknowledging the baseline's subscription economics are themselves unverified post-2026-01-09 (§5, §7-#14) — and run a time-boxed opencode spike whose first three gates are decisive, only when a concrete self-hosted-model requirement appears and the user accepts measured structured-output degradation.

---
---

# Round 4 — Positioning `rig` (Rust-native LLM library) in the harness decision

> Prior verdict (R1–R3): **stay on baseline (Claude Agent SDK)**; **opencode** = conditional TS challenger; **Pi disqualified**. This round folds in **rig** (`rig-core` v0.38.1, 0xPlaygrounds, MIT, ~7.5k★). All claims below are source- or closed-issue-verified (verify phase 14/14); where a verifier verdict contradicts a raw finding, the verdict wins. **Evidence grades:** **[CI]** closed-issue-verified, **[SRC]** source-verified, **[DOC]** docs-only / unverified-in-the-wild.

## 1. TL;DR — updated recommendation

**rig becomes a genuine co-recommendation to baseline — and the strategic long-term choice *if and only if* the two deciding variables below favor it.** For a Claude-only present on subscription economics, baseline still wins on risk. For RightClaw's stated identity (Rust shop, values control, accepts build cost, self-host ambition), rig is the only option that actually *wires* the levers R3 proved the TS harnesses could not pull.

**Confidence: medium-high** on the capability findings (source + closed-issue verified), **medium** on the verdict (the deciding variables are product/business questions, not rig questions).

**Two deciding variables, not one:**

1. **Is self-host / multi-model a committed requirement, or a someday-maybe?**
   - **Committed →** rig is the **recommendation**. It is the only option (baseline is Claude-locked; opencode's structured output is emulated and its caching is inert off-Anthropic) that lets RightClaw enforce `response_format` json_schema **[SRC]** and control Anthropic `cache_control` **[SRC+CI]** at the request layer. Building the harness in Rust also erases R1–R2's core pain (no JS runtime, no `node_modules`, no sidecar trust boundary).
   - **Someday-maybe →** rig is a **co-equal second**. The pre-1.0 churn (#1561 **[CI]**) and the dominant self-host-gateway bug class (#1829, #1362, #1085, #1440 **[CI]**) are real costs you'd pay *now* for a benefit you'd realize *later*. Baseline stays the lower-risk default while on Claude.

2. **API-billing vs subscription economics.** rig is **API-key auth [SRC]**; it does *not* restore baseline's subscription economics, and the 2026-01-09 Anthropic third-party subscription-OAuth block does not change that calculus. If moving to rig forces API billing, the cost delta (API-billed rig vs whatever subscription path the baseline retains) can *itself* be decisive — co-equal with the self-host question. Needs explicit cost modeling before committing.

**Decisive action:** a time-boxed harness spike (one vertical slice: agent loop + MCP wiring + enforced structured output against the *actual* target vLLM build), **not** an immediate full migration.

What rig does **not** do, and must not be hyped past:
- It does **not** make a weak self-host model enforce schema — enforcement strength is the *provider's* (#1031 **[CI]**). rig exposes the lever; the model honors it or doesn't.
- It still has a **fail-fast violation in the streaming path** (#1829 **[CI]**): on SSE parse error it logs and returns `Ok(None)`, silently dropping the tool call. The fix PR (#1822) widened the deserializer but the `Ok(None)` swallow itself **remains on main** (verified). RightClaw's fail-fast rule (AGENTS.rust.md §2) forbids this — a guard wrapper is mandatory if you build on rig.

## 2. rig capability map

Legend — **Provided:** `rig` (free primitive) / `RC builds` (RightClaw owns) / `model-dep` (provider/model property). **Support:** ✅ shipping / ⚠️ partial-or-caveated. **Evidence:** [CI]/[SRC]/[DOC].

| Load-bearing capability | Provided by | Support | Ev | Killer caveat (cite) |
|---|---|---|---|---|
| Multi-turn agentic loop (`max_turns`, `MaxTurnsError`) | rig | ✅ | [SRC] | **Default `max_turns=0` = ONE tool round-trip**; must set explicitly. (C1/C2) |
| Tool-call interception (PromptHook: `on_tool_call`, `on_invalid_tool_call`, streaming deltas) | rig | ✅ | [SRC+CI] | Hook API churns: `CancelSignal→HookAction` (#1304), arg-mutation `ContinueWith` only added 2026-05 (#1680). (C3) |
| Native structured output → OpenAI `response_format` json_schema (`output_schema`, `strict:true`) | rig | ✅ | [SRC+CI] | Suppressed on **first tool turn** until history has a tool_result. RC loop must order. (C6; PR #1382/#1378) |
| Raw provider passthrough (`additional_params` → body, vLLM `guided_json`/`extra_body`) | rig | ⚠️ | [SRC] | **Shallow top-level merge** — nested keys replace wholesale; hand-shape body per gateway. **No closed issue tests `guided_json`.** (C5) |
| Anthropic `cache_control` (manual/auto/1h/raw, 4-breakpoint budget) | rig | ✅ | [SRC+CI] | Manual mode breakpoints last message every turn → can churn on growing transcript; min-token floors silently skip. (C8; #1811,#1297) |
| MCP client (rmcp 1.7, `McpTool`+`McpClientHandler`, StreamableHTTP, list_changed refresh) | rig | ✅ | [SRC+CI] | Recovery single-attempt (#1522); RC owns retry/health. rmcp churn 0.8→1.7. (C9) |
| MCP Bearer auth to aggregator (`auth_header`) | rig (rmcp) | ⚠️ | **[DOC]** | **rotation × `reinit_on_expired_session` UNVERIFIED** — clear in spike. (open-q #5) |
| Subagents (agent-as-tool: `impl Tool for Agent<M>`) | rig (mech) / RC builds (policy+isolation) | ⚠️ | [SRC] | Handoff mechanism only; **no CC-style isolation model**. |
| Tool restrict/disable per turn (`allowed_tools`) | rig (enforce) / RC builds (policy) | ✅ | [SRC] | Maps cleanly to SKILL.md `allowed-tools` (the opencode gap). |
| Turn caps + per-call usage | rig | ⚠️ | [SRC+CI] | `max_turns` primitive; **USD budget cap is RC-built**; retry-on-failure **wontfix** (#782). |
| Session persist / resume / fork | RC builds | ✅ | [SRC] | rig stateless: `chat_history: Vec<Message>` Serde → persist=serialize, fork=clone-branch. Trivial but RC's. |
| Idle compaction | rig-memory (transform) / RC builds (trigger) | ⚠️ | [SRC] | `Compactor` exists but RC's `/compact`-by-exit-status model **changes semantics**. |
| Reflection | RC builds | ✅ | [SRC] | Trivial on stateless history. |
| Composite system prompt | RC builds (rig: preamble blocks) | ✅ | [SRC] | `preamble`/`append_preamble`; RC composes. |
| SKILL.md discovery/exec + learning pipeline | RC builds | — | [CI] | **Entirely RC's** — maintainer skeptical skills in scope (#1033). |
| NDJSON stream logging | RC builds (atop OTel spans) | ✅ | [SRC] | rig emits OTel GenAI spans incl. cache tokens; NDJSON sits on spans, not stdout-scraping. |
| Per-agent auth | RC builds | ✅ | [SRC] | One rig client/agent per key; trivial. |
| Off-Claude tool-calling / enforcement **reliability** | model-dep | ⚠️ | [CI] | Not rig's: vLLM double-stringify (#1085), 2nd-turn 400 (#1362→#1367), reasoning dropped (#1440), local extractor fails (#1031). |

**Net:** the *correctness levers* (loop, hooks, structured output, cache, MCP) are **free primitives**, mostly [SRC+CI]. The *harness identity* (skills, learning, sessions, reflection, NDJSON) is **RC-built but well-supported by the primitives**. The *reliability ceiling off-Claude* is **model/provider-bound, not rig-bound**. The one **[DOC]-only** load-bearing claim is MCP Bearer-auth rotation (§4).

## 3. Does rig fix R3's reliability gaps? — the crux

**Yes for the levers, with one grade split: OpenAI `response_format` path confirmed; vLLM `guided_json` reachable-but-unverified. The ceiling stays model-bound.**

**(a) Enforced structured output on self-host — `response_format` FIXED [SRC]; `guided_json` REACHABLE-BUT-UNVERIFIED [SRC, zero CI].**
- **OpenAI-native `response_format` — confirmed [SRC].** rig has native `output_schema: Option<schemars::Schema>` on `CompletionRequest`; the OpenAI-compatible path emits `response_format:{type:json_schema, json_schema:{... strict:true}}` into the **real request body** (verdict C6; PR #1382 fixing #1378, with passing unit tests). *Enforced*, not the synthetic-submit-tool emulation opencode's typed SDK was stuck with. Ollama maps `output_schema`→grammar-constrained `format`. **For any gateway that speaks OpenAI `response_format`, the lever is genuinely wired.**
- **vLLM `guided_json`/`extra_body` — escape hatch exists, NOT verified in the wild [SRC, zero CI].** `additional_params` injects arbitrary body fields (C5) **but** the merge is **shallow top-level** — `guided_json` is top-level in native vLLM but nested under `extra_body` in OpenAI-SDK-style gateways, so a naive nested insert *replaces wholesale*; **RC must hand-shape the exact body per gateway.** **Zero closed issues exercise `guided_json`** — the least-evidenced load-bearing claim in the report; this is the **#1 spike item**.
- **The model still has to honor it** (#1031 [CI]): local LMStudio/qwen/gpt-oss fail enforcement regardless. rig wires the lever; enforcement strength is the provider's — R3's framework-independent reality, intact.

**(b) Anthropic `cache_control` — FIXED, most complete surface of any option [SRC+CI].** Verified in `anthropic/completion.rs` (C8, all four sub-claims): `with_prompt_caching()`, `with_automatic_caching()`/`_1h()`, raw `additional_params.cache_control`, **4-breakpoint budget** (`MAX_CACHE_CONTROL_MARKERS=4`), `cache_control` on `ToolDefinition` (#1811), cached-token accounting (#1297). Exactly the lever opencode lost and the native `claude` CLI currently owns. **Highest evidence grade in the report.** Caveat: manual mode rebreakpoints the last message every turn → can churn on a growing transcript; align RC's stable-prefix assembly with rig's breakpoint placement. Spike item.

**Bottom line:** caching gap → **yes, shipping tested code**; structured-output gap → **yes for `response_format`, unverified-yes for `guided_json`**. This split is the strongest honest argument for rig over opencode, and why rig — not opencode — is the self-host challenger.

## 4. MCP — usable for RightClaw's aggregator?

**Client: yes, directly usable [SRC+CI]. Bearer auth: API exists but rotation interplay UNVERIFIED [DOC].**
- **Client — confirmed [SRC+CI].** Real MCP client on official `rmcp` 1.7 (`client` feature): `McpTool` adapts rmcp tools into rig's `ToolDyn`; `McpClientHandler::connect` lists+registers into a shared `ToolServer`, auto-refetches on `notifications/tools/list_changed` (#1521). Verified `tool/rmcp.rs` (C9).
- **Transport matches RC's aggregator exactly [SRC].** `StreamableHttpClientTransport::from_uri` — the same Streamable-HTTP RC exposes at `:8100/mcp`. Tools flow in automatically — RC does **not** re-declare its `mcp__right__*` tools; connect the transport and the set flows in. Cleanest possible fit.
- **Per-agent Bearer — API exists, rotation UNVERIFIED [DOC].** rmcp 1.7 `StreamableHttpClientTransportConfig` exposes `auth_header`, `custom_headers`, `reinit_on_expired_session` (docs.rs only — no closed issue/verdict/compile-check). Critical unknown: **does `auth_header` survive `reinit_on_expired_session`, or must RC rebuild the transport on rotation?** (open-q #5). Treat as "works per docs, rotation unproven."
- Other caveats (non-disqualifying): recovery single-attempt (#1522 — RC keeps `SandboxSupervisor`); rmcp churn (#941→#1595); **UNKNOWN** shared-`ToolServer` concurrency defect (#1573 read-lock across `tool.call().await`) → RC should give **each agent its own `ToolServer`**; confirm fix landed.

## 5. Deployment & security — the Rust-native advantage

| Option | In the sandbox | Supply-chain | Trust boundary |
|---|---|---|---|
| Baseline (Claude Agent SDK) | Node 20 + `node_modules` sidecar spawning native `claude` | Large (npm) | Host↔sidecar ingress (R1–R2 pain) |
| opencode | TS in-sandbox server | Large (npm) | In-sandbox HTTP server |
| **rig (shape b)** | **Single static musl `right-agent-runner`** embedding rig+rmcp | **Zero JS; one Rust binary** | Thin host-bot↔runner over `exec_in_sandbox` |

**Recommended shape (b) — *contingent on a successful musl-link spike*.** A static musl `right-agent-runner` embedding the rig loop + MCP client, deployed inside the sandbox, driven from the host bot. *If the link succeeds*, this is the **cleanest of all options**: loop+tools+model calls colocated in the box, zero JS supply chain, smallest attack surface. `invocation.rs` becomes a *runner-invocation* builder; `guard_no_sandboxed_host_exec` still applies.

> **Preconditions gating the recommendation (not footnotes):**
> - rig defaults to **rustls** (good for static musl — C19 [SRC]), but a **full static-musl link of the whole graph** (rig+rmcp+reqwest/rustls+tokio+turso/sqlite) is **UNVERIFIED**. Cross-compile spike must succeed before committing shape (b).
> - The **host↔runner protocol is net-new surface** (UNSPECIFIED): transport choice + how it carries resume/fork/streaming/structured-output. Real, unsized engineering `invocation.rs` absorbs.

**The host-loop anti-pattern — name it and refuse it.** Because rig is a library, the tempting shortcut is to run the loop **on the host** and forward only tool calls into the sandbox. That **breaks the security model** (loop + model credentials outside the box). If you adopt rig, the loop **must** live in the in-sandbox runner (shape b).

## 6. Maturity / risk

**Two real risks: pre-1.0 churn, and a self-host-gateway bug class on exactly RC's surface.**
- **Backing & bus factor (C16/17/23):** 0xPlaygrounds — small org, no disclosed funding, but **better-than-single-maintainer**: top contributors joshua-mo-143 (249), cvauclair (160), +4 each >50; Dependabot + release-plz. Named prod users: ilert (#1688 — "Rig powers our internal LLM proxy that fronts every agentic workflow"), Dria. Far healthier than Pi's single-maintainer fork; far below Anthropic.
- **Cadence & churn (C15/20 [CI]):** pre-1.0 v0.38.1, ~38 minors/24mo. On 0.x each minor can break. #1561 *"YOLO mode … impossible to keep up with breaking changes"* (closed **duplicate** = repeated), #628 SemVer request. Concrete breaks: `max_depth→max_turns` (#1310), rmcp bumps, client consolidation (#911), Chat-history append (#1733 breaking). **Requires pin-and-vendor strategy** (§8).
- **Dominant closed-bug class = RC's exact surface (C12/13/14/21 [CI]):** (1) multi-turn history correctness across tool round-trips — assistant/reasoning content dropped → provider rejection (#1614,#1559,#1434,#1642); all fixed but recur. (2) OpenAI-compat/self-host gateway brittleness — strict enum crash (#1729 `service_tier`), object-vs-string streamed tool args (#1829), vLLM 2nd-turn 400 (#1362→#1367), Granite double-stringify (#1085), reasoning dropped (#1440). **Precisely where RC's vLLM ambition lands.** (3) **Fail-fast violation survives:** #1829 fix left `Ok(None)` swallow on main (`streaming.rs` L172-176) → RC must wrap stream path to treat empty-assistant-after-tool-call as an error. Non-negotiable (AGENTS.rust.md §2).
- **Mitigant (C22 [CI]):** fix latency good — #1829/#1827/#1836/#1843 closed within days; #1031 fixed within a week. Offsets churn, not bus factor.
- **Meta-risk:** building the entire harness on a young, high-churn library — every hook-API/message-shape break ripples into RC. Payable with pinning + a wrapper layer, not free; the strongest argument for keeping baseline as default until self-host is committed.

## 7. Migration architecture & what gets rebuilt

**Chokepoint inverts:** `ClaudeInvocation` (`crates/bot/src/cc/invocation.rs`) stops building a `claude` argv and builds a **runner invocation** — params to the in-sandbox `right-agent-runner` over the host↔runner protocol. Invariant-enforcement role survives (composite prompt, MCP config, structured-output schema, budget/turn caps → runner params); `guard_no_sandboxed_host_exec` still gates it.

**rig gives free/near-free:** agentic loop (C1), tool hooks (C3), MCP client (C9), enforced `response_format` (C6), cache_control (C8), `allowed_tools` enforcement (C11), OTel spans, ~25 provider adapters, agent-as-tool subagent mechanism.

**RC rebuilds (harness level), rough size:** SKILL.md exec + `allowed-tools` mapping (medium); **per-turn learning pipeline** (large — most complex); session persist/resume/fork (small-med); idle compaction (medium, **semantics change**); reflection (small); retry/backoff (carry forward, #782 wontfix); USD budget cap (small-med, abort via hook); NDJSON on OTel spans (small-med); **host↔runner protocol + static-musl runner (net-new, unsized — biggest unknown)**; fail-fast wrapper for #1829 (small, mandatory).

**Multi-model/self-host landing:** one rig client per RC agent, per-credential `base_url` override pointed at vLLM. Schema via native `output_schema`→`response_format` (confirmed) or `additional_params`→`guided_json` (unverified — spike). Cache via `with_prompt_caching()` on Anthropic-direct (confirmed). No total line-count estimate exists; a one-slice spike must calibrate.

## 8. Open questions / spike items

**Spike (one vertical slice vs the *actual* target vLLM):** (1) **static-musl link** of the full graph — UNVERIFIED, blocks shape (b); (2) **`guided_json` body shape** through the shallow merge (top-level vs nested `extra_body`) — #1 evidence gap; (3) `additional_params` per-turn persistence (every call or just first?); (4) vLLM streaming tool-call robustness post-#1829 (Value-tolerant or still strict-enum #1729?); (5) **Bearer rotation × `reinit_on_expired_session`** (resolves the [DOC] gap in §4).
**Design/verification:** (6) `ToolServer` concurrency #1573 (lean per-agent); (7) cache breakpoint churn on long sessions (align stable-prefix assembly); (8) idle-compaction re-model vs sub-invocation; (9) USD mid-run abort via `on_completion_response`; (10) subagent isolation vs `bgIsolation`; (11) fail-fast wrapper for #1829 (mandatory).
**Governance:** (12) pin/vendor strategy for pre-1.0 churn — decide before committing.
**Business:** (13) API-billing vs subscription economics — model the cost delta explicitly (co-equal deciding variable).

## Final ranking & conditions

1. **Baseline (Claude Agent SDK)** — wins when RightClaw stays Claude-only, values subscription economics, self-host is someday-maybe. Lowest risk; Anthropic-backed; no harness rebuild. Default until both deciding variables move.
2. **rig** — wins when self-host/multi-model is a *committed* requirement **and** API-billing is acceptable. The only option that genuinely *wires* enforced `response_format` + Anthropic `cache_control`, in Rust, loop in the box. Cost: own the whole harness, absorb pre-1.0 churn + a self-host-gateway bug class on your exact surface, wrap one live fail-fast violation, and clear the musl-link + `guided_json` + Bearer-rotation unknowns in a spike first.
3. **opencode** — third. Emulated structured output (typed SDK omits `format`), caching inert off-Anthropic, TS supply chain. No reason to prefer over rig for a Rust shop, or over baseline for a Claude shop.
4. **Pi** — remains disqualified (R3).

**One line:** rig is the first option that *fixes* R3's reliability gaps (caching: confirmed; `response_format`: confirmed; `guided_json`: reachable-but-unverified) rather than working around them, and the only one that does so in Rust with the loop in the box — but recommend it over baseline **only after** a time-boxed spike clears the three amber unknowns (musl link, `guided_json` body shape, Bearer-rotation) **and** both deciding variables (committed self-host, acceptable API billing) favor it. Otherwise baseline stays the lower-risk default with rig as the documented strategic path.


---
---

# Round 5 — Subscription-OAuth feasibility & final rig architecture

> **Reliability note for this round:** the verify phase fully failed (all 14 fact-checkers did not return structured output) and the synth draft hit a rate-limit; the final report below was produced by the synthesis agent's own skepticism + the research findings, **without independent adversarial verification**. It is included because its central conclusion is the *conservative* one (the attribution gate is UNKNOWN → lead with the hybrid fallback), and it self-caught a fabricated quote and contradictory citations. Treat specific sub-claims as lower-confidence than rounds 1/3/4; the go/no-go spike sequence (§6–§7) is what to trust and act on.

# Round-5 Final Report — rig as RightClaw's Harness: Subscription-OAuth Attribution, Feasibility, Economics, Architecture

**Decision posture:** The make-or-break question (Q1, attribution gate) is **UNKNOWN for a custom rig client** — no primary source resolves it, and the five research passes reached three mutually exclusive answers. This report therefore **leads with the hybrid fallback as the de-risked default** and treats rig-native-Claude as a spike with material technical and ToS risk. The rig decision itself (Rust-native harness, self-host via OpenAI-compatible providers, rmcp MCP client) stands; only the **Claude auth sub-path** is unresolved.

---

## 0. What changed from the draft (critique integrated)

- The draft was an `API Error … Rate limited` — there was no synthesized report. This is it.
- **Deleted the fabricated #15080 maintainer/teknium1 quote** (former C6: "Closing — this is intended behavior… overage bucket exclusively"). I verified the critique's claim posture: that authoritative-maintainer interpretation is **not citable**. Downgraded to: *one reporter observed tools-present 400s; cause and Anthropic intent unconfirmed.*
- **Bearer-vs-x-api-key is now a live spike, not a fact.** The evidence asserts it both ways; primary docs favor Bearer; mark unresolved.
- **cache_control and structured output on the OAuth-fork path demoted to UNVERIFIED.** Confirmed only on api-key/official paths.
- **Economics made conditional on Q1** and flagged as unmeasured (3× spread across analysts).
- **ToS / identity-spoofing elevated to a top-level risk.**
- **"Contribute back to rig" reframed** as: surgical for non-spoofing parts, improbable upstream for the identity-spoof layer → plan for a vendored fork.

---

## 1. Attribution gate (Q1) — UNKNOWN for a custom client; this is the crux

**The honest position: no primary source confirms that a custom rig client presenting a subscription OAuth token draws from *either* the Agent-SDK credit pool *or* the interactive bucket on tools-bearing turns.** The supported, documented path is "third-party apps that authenticate **through the Agent SDK**." Everything beyond that phrasing is inference.

### 1.1 What IS supported by primary sources

| Fact | Evidence | Quote |
|---|---|---|
| From 15 Jun 2026, `claude -p` + Agent SDK subscription usage draws a separate monthly credit | code.claude.com/docs/en/authentication | "Starting June 15, 2026, Agent SDK and `claude -p` usage on subscription plans will draw from a new monthly Agent SDK credit, separate from your interactive usage limits." |
| Credit covers third-party apps **authenticating through the Agent SDK** | support.claude.com/articles/15036540 | "Third-party apps that authenticate with your Claude subscription through the Agent SDK" |
| Amounts $20 Pro / $100 Max5x / $200 Max20x, per-user, no rollover, API rates, overflow→API or stop | support.claude.com/articles/15036540 | "additional Agent SDK usage flows to usage credits at standard API rates—but only if you've enabled usage credits. If usage credits aren't enabled, Agent SDK requests stop until your credit refreshes." |
| `claude setup-token` mints a 1-year OAuth token, inference-scoped | code.claude.com/docs/en/authentication | "This token authenticates with your Claude subscription… It is scoped to inference only" |
| Raw OAuth on the Messages API was **rejected and the fix declined** | github.com/anthropics/claude-code/issues/37205 | Closed **not planned / invalid**; "OAuth authentication is currently not supported." |

The `claude -p` note and the support article both describe **Anthropic's own binary and "through the Agent SDK."** Neither addresses a custom rig client. **support.claude.com is silent on whether a non-official client presenting subscription OAuth qualifies** — that is the correct, conservative reading.

### 1.2 Where the research passes contradict each other (do not resolve in code)

Three incompatible answers, each from weak/secondary or now-suspect evidence:

- **"Custom + tools → overage/400, official binary recognized, third-party not"** — rested partly on a **fabricated** #15080 maintainer quote (deleted). The *experiment* (tools=True → 400, tools=False → 200) appears real but is **one reporter's observation, pre-June-15, cause unconfirmed**. The two passes citing it quote *different* "verbatim" strings for the same issue → treat both as paraphrase.
- **"Auth mechanism decides; OAuth + agent loop → interactive bucket (cheaper than the pool)"** — rests on a **single secondary blog (fazm.ai)**, not a primary Anthropic source: *"If the wrapper logs in with your Claude subscription via OAuth … its usage is billed like interactive Claude Code: against your plan's normal limits."* If true, the fork is *better* than the brief assumed. Unverified.
- **Brief's own premise: OAuth → the credit pool.** Also unproven for a custom client.

**These cannot all be true. The report's position is: undetermined.** The fazm.ai claim *directly contradicts* the support-doc framing; nobody outside Anthropic demonstrably knows the rule for a header-spoofing custom client post-June-15.

### 1.3 The OAuth wire mechanics are themselves contradictory in the evidence

- One pass: OAuth must go via `x-api-key`, **Bearer rejected** (cites `earendil-works/pi#2751`).
- Another: same issue number cited as `badlogic/pi-mono#2751` — **one URL is wrong; neither verified as quoted.**
- Primary docs say `ANTHROPIC_AUTH_TOKEN`/`CLAUDE_CODE_OAUTH_TOKEN` → **`Authorization: Bearer`** ("Sent as the `Authorization: Bearer` header").
- Other passes insist Bearer is required and x-api-key is wrong.

**Conclusion: the exact header transport for OAuth-on-Messages-API is UNRESOLVED. Spike it; do not hardcode either form as fact.** The OAuth-client-id / PKCE / `claude.ai/v1/oauth/token` refresh-endpoint details (from community gists) are plausible but secondary — verify against a live `setup-token` flow.

### 1.4 Verdict for Q1

**UNKNOWN.** The only client *proven* to keep subscription/credit attribution with tools is **Anthropic's own binary**. A rig fork is a real spike with material risk that the working path requires **deliberate first-party-client impersonation** (headers + system-prompt prefix), which Anthropic detects, **declined to support (#37205)**, and can revoke. → **Rank the hybrid fallback first.**

---

## 2. rig feasibility (Q2) — the *mechanics* are feasible; the *attribution* is not proven

The critique is right that "rig can do X" was conflated with "X works on the custom OAuth fork." Separating them:

### 2.1 Confirmed in rig source (mechanics — these are solid)

- **Header injection needs no core fork:** `ClientBuilder::http_headers(HeaderMap)` is public; every request copies client default headers before the provider's `with_custom(req)` hook (`crates/rig-core/src/client/mod.rs`). An injected `Authorization`/`anthropic-beta`/`anthropic-version` reaches `/v1/messages`.
- **x-api-key is conditional:** inserted only `if !headers.contains_key(&k)` — a pre-set `Authorization` is not clobbered.
- **Auth swap is a one-line fork:** `AnthropicKey::into_header()` hardcodes `x-api-key` (`providers/anthropic/client.rs`). To emit Bearer instead requires forking *that one site* (or sidestepping it via `http_headers` + suppressing the key).
- **Precedent exists in-tree:** `providers/chatgpt` is a **ChatGPT subscription-OAuth provider** (merged PR #1615, 2026-04-10) with ext-owned `Authenticator`, per-request `Authorization: Bearer`, device-flow login, **token refresh** (`auth/native.rs`). A subscription-Anthropic provider is a direct analog — **a new in-tree provider, not a core fork** (for the non-spoofing parts).
- **Per-agent dynamic auth is cheap:** token resolved per-request; build a distinct client/ext per agent token. (Note: headers are `Arc`-captured at build → **token rotation = rebuild the client**, not mutate in place.)
- **Completion posts to `/v1/messages` via the shared client** → injected headers apply.

### 2.2 Levers on the OAuth-fork path — UNVERIFIED (critique fix)

- **cache_control:** rig supports it (`anthropic_beta("prompt-caching-…")`, manual+auto breakpoints), and Claude Code on a subscription uses 1h caching. **Neither proves a custom fork's `cache_control` is honored/billed-as-expected on the OAuth wire.** → `partial / spike`.
- **Structured output:** rig has native Anthropic `OutputFormat::JsonSchema` on `/v1/messages`; needs `anthropic-beta: structured-outputs-2025-11-13`. **No source shows it works on the subscription-OAuth path; no api-key-gating evidence either way.** → `supported-pending-spike`.

**So: both load-bearing levers are confirmed only on api-key/official paths.** Any claim they survive the fork's OAuth path is currently unsupported.

### 2.3 No rig precedent for Anthropic-OAuth

No closed rig issue/PR tracks Anthropic subscription/Claude-Code OAuth. The user's fork would be the first. The hermes-agent #25267 "Codex-style subscription OAuth" precedent is an **OPEN, unassigned, low-priority feature request with no maintainer verdict** — a *requested* pattern, not proven engineering.

---

## 3. Economics (Q3) — conditional on Q1; honest verdict = prepaid full-rate API

**This entire section is moot if Q1 lands on "interactive bucket" (then it's the old flat economics) instead of "credit pool."** Present it as conditional.

### 3.1 Grounded facts

Pricing (platform.claude.com/docs pricing, verified consistent across passes): **Opus $5 in / $25 out; Sonnet $3 / $15; cache read 0.1×; 5-min cache write 1.25×.** Credit is **prepaid full-rate API dollars**, per-user, no rollover, then overflow→API (if enabled) or stop.

### 3.2 The numbers disagree 3× and are all unmeasured

| Analyst | Opus $/turn | Opus turns/mo on $200 |
|---|---|---|
| Pass A | ~$0.17 | ~1,165 |
| Pass B | ~$0.05–0.08 | ~2,500–4,000 |

That spread comes entirely from **unmeasured token assumptions**, despite RightClaw having real NDJSON stream logs (`~/.right/logs/streams/<uuid>.ndjson`) that would settle it. **Neither figure is a planning number.** Before relying on economics, compute one model from **measured** RightClaw composite-prompt + MCP-tool-def + cache-read token counts.

### 3.3 Honest verdict

The credit **is** prepaid API dollars at list price. Past the cap it **is** API pricing. Benefit = a fixed monthly free pool ($20/$100/$200/agent) + caching — **real for low-volume chat agents, collapses to API pricing at volume** (a busy cron+chat agent exhausts Max20x in days). **Drop the "preserves cheap subscription economics" framing.** Prompt caching (≈90% off repeated input) is the single biggest stretching lever — *if* it works on the chosen auth path (§2.2, unverified for the fork).

---

## 4. Architecture finalization (Q4) — one rig harness for self-host; Claude auth path is the variable

**Self-host through rig: settled.** One rig-native runner serves OpenAI-compatible vLLM providers, owns the rmcp StreamableHTTP MCP client to RightClaw's aggregator, and (per the critique-corrected musl note) the **runner links only rig + rmcp + reqwest/rustls + tokio** — turso/rusqlite (the hard C-link) stay in the host bot/`right-db`, shrinking the static-musl risk.

**Per-agent provider selection: enum dispatch, not dyn.** `CompletionModel` is a compile-time generic with associated `Response` types + a `Clone` bound — **not object-safe**; no `Box<dyn CompletionModel>`, no `DynClientBuilder`. RightClaw must hand-write `enum AgentModel { AnthropicOAuth(...), Vllm(...) }` with an `impl CompletionModel`. This cleanly spans both paths.

**Claude path — DO NOT finalize as rig-native yet.** Whether one rig harness can natively serve Claude depends entirely on Q1:

- **If the spike shows the fork lands on a usable bucket AND the levers survive (§2.2):** rig-native Claude is preferred — it keeps request-layer `cache_control` and enforced structured output that routing through the official binary/ACP would partially forfeit.
- **If Q1 fails or is ToS-blocked:** **hybrid is mandatory** — route Claude through the official `claude` binary / Agent SDK (preserves credit attribution; loses some rig levers), self-host through rig. The rig harness still owns self-host + MCP; only Claude turns delegate.

**Therefore the architecture is "rig-native for self-host + MCP, Claude auth sub-path TBD by spike," not "rig-native for everything."**

---

## 5. ToS / identity-spoofing risk (elevated to top-level — critique fix)

The *working* fork path (if it exists) requires, per the evidence: (a) swap auth header, (b) inject Claude-Code identity headers (`claude-code-20250219`, `oauth-2025-04-20`, `x-app: cli`, `claude-cli/…` UA), (c) **prepend Claude Code's static system-prompt prefix** (server-side prompt-content pattern-matching reported), (d) an OAuth refresh loop, (e) per-agent client rebuild.

Point (c) is **deliberate first-party-client impersonation.** Anthropic detects system-prompt content, **declined to support raw-OAuth Messages-API use (#37205)**, and community sources note ToS language that subscription OAuth tokens are "for official clients." OpenClaw reports staff said the *usage* is "allowed again," but that does not bless *impersonating the official client*. For a "closed-box, security-first" platform, **this is a lead-with risk, not a footnote** — and untested for RightClaw's exact composite prompt (Claude-Code prefix + agent identity + MCP instructions + memory), which may trip the detector.

**"Contribute back to rig":** plausible/surgical for the non-spoofing parts (Bearer, betas, base_url, refresh). **Improbable upstream for the identity-spoof layer** — 0xPlaygrounds is unlikely to merge an impersonation provider into rig-core. **Plan for a vendored patched rig-core,** not an upstreamed feature.

---

## 6. Consolidated spikes (round-4 carryover + new attribution spike)

| # | Spike | Go/No-Go test |
|---|---|---|
| **S1 (decisive)** | **Subscription-OAuth attribution** | **Post-June-15, throwaway Pro/Max account, usage-credits DISABLED.** Send one tools-bearing `/v1/messages` turn via the fork (both Bearer and x-api-key transports; with full CLI identity headers + Claude-Code system prefix). Read the **billing dashboard** to see which meter moved: interactive vs credit vs overage-400. **Disabled usage-credits is the discriminator** — without it you cannot distinguish "credit pool" from "silent API overflow." Also run the tools=on/off A/B to confirm/refute the reclassification report. |
| **S2** | **Levers on the OAuth path** | On the same fork: confirm `cache_control` is honored/billed-as-cached, and `structured-outputs-2025-11-13` + `json_schema` returns 200 (not 400/ignored) on the OAuth token. |
| **S3** | **musl static link (runner-only graph)** | `cross build --target x86_64-unknown-linux-musl --release` of the runner = rig + rmcp + reqwest(default-features=false, rustls-tls) + tokio, **excluding turso/rusqlite**. `ldd` must report "not a dynamic executable." Pin ring vs aws-lc-rs explicitly. |
| **S4** | **vLLM guided_json body shape** | Capture the live request body; assert top-level `guided_json` (native vLLM) vs nested `extra_body.guided_json`. rig's `additional_params` merge is **shallow top-level** (`json_utils.rs`) → a sibling `extra_body` is clobbered; an OpenAI-style gateway needs a deep-merge shim. |
| **S5** | **MCP Bearer rotation × reinit** | rmcp `StreamableHttp` stores `auth_header` in `Config` and re-passes it through `perform_reinitialization` (survives reinit) but it's a **static snapshot** → rotation = rebuild transport. Add a **runtime assertion test**, not just a doc note. |
| **S6** | **#1829 Ok(None) fail-fast wrapper** | rig's SSE parser returns `Ok(None)` on parse error (chunk dropped, error swallowed). Wrap to convert to a hard `Err` per AGENTS.rust.md FAIL-FAST. Upstream fix is a **separate open PR (#1822-class)** — do not assume it lands. |

---

## 7. Go/No-Go sequence — prove these, in order, before committing to build the rig harness

1. **S1 — attribution spike (decisive).** Run it first; nothing else about the Claude path matters until the meter is known. **Gate:** does a custom fork land on a usable subscription bucket (interactive or credit) for tools-bearing turns, with usage-credits disabled?
   - **PASS →** proceed to S2.
   - **FAIL / overage-400 / requires impersonation you won't ship →** **STOP the fork plan. Adopt the hybrid:** Claude via official `claude`/Agent SDK, self-host via rig. (Architecture §4 already supports this; only the Claude sub-path changes.)
2. **S2 — levers on the OAuth path.** Only meaningful if S1 passed. **Gate:** do `cache_control` + structured output work on the OAuth wire? If not, rig-native-Claude loses its advantage over the official binary → **prefer hybrid** even if S1 nominally passed.
3. **S3 — musl link of the runner-only graph.** Independent of S1/S2; needed for *any* in-sandbox rig runner (self-host path too). **Gate:** fully static binary. If it fails, the deployment shape needs rework regardless of the Claude decision.

S4–S6 are implementation-hardening, not go/no-go for the harness decision — schedule after S1–S3.

**Hybrid fallback (if the attribution gate fails):** rig owns self-host vLLM providers + the rmcp MCP client + per-agent enum dispatch; **Claude turns route through the official `claude` binary / Agent SDK** (today's path, with confirmed credit attribution). You lose programmatic `cache_control`/structured-output control on the Claude branch but keep a supported, non-ToS-exposed, non-impersonating path. This is the **de-risked default** until S1+S2 prove otherwise.

---

**Bottom line:** rig is the right harness for the Rust-native, self-host, MCP requirements. The **subscription-OAuth-for-Claude fork is an unproven, ToS-exposed spike**, not a settled win — the attribution gate is genuinely UNKNOWN, the working path likely requires first-party-client impersonation, and the two levers the user is counting on (`cache_control`, structured output) are unverified on that path. **Build rig for self-host + MCP now; prove S1→S2→S3 before committing rig-native Claude; ship the hybrid Claude path as the default fallback.**

---
---

# Round 5 — ADDENDUM (2026-06-05): official @ClaudeDevs confirmation resolves the attribution gate

> **Source the user supplied: official @ClaudeDevs tweet, 2026-05-13** (https://x.com/ClaudeDevs/status/2054610152817619388). This is a PRIMARY Anthropic source that supersedes round 5's "UNKNOWN" on Q1.

**Decisive quote:** *"This means that third-party tools built on the Agent SDK like Conductor and OpenClaw work with your Claude plan, but will draw from your credit **the same way your own scripts do**."* Plus the headline: the dedicated monthly credit covers *"Third-party apps built on the Agent SDK."*

**What this changes:**
- **Q1 (attribution gate): re-graded from UNKNOWN → OFFICIALLY SUPPORTED for third-party tools + own scripts.** Programmatic subscription usage drawing from the credit pool is explicitly sanctioned for third-party tools (Anthropic names **OpenClaw** and **Conductor**) and for "your own scripts." The "same way your own scripts do" phrasing points to a **token-determined** model (the subscription credential drives the pool), not a strictly-official-binary gate.
- **Round 5's ToS / first-party-impersonation alarm is substantially DEFLATED.** Anthropic publicly blesses non-Anthropic third-party tools (incl. OpenClaw, a peer-ecosystem harness) using Claude subscriptions programmatically. The "deliberate impersonation" framing was too strong.
- **The block claim (rounds 3–4) is doubly dead** — this is the official, post-policy framing: not blocked, metered via the credit.

**What's still NOT fully resolved (the narrow residual):**
- The phrase **"built on the Agent SDK"** vs round-5's **#37205 ("OAuth not supported on the Messages API")**. Open question: do OpenClaw/Conductor hit the **raw Messages API with subscription OAuth** (→ token-determined → rig-native works), or do they **wrap/route through the official Agent SDK** (→ a rig client must route through the SDK pathway, not speak raw `/v1/messages`)? This is the last bit that confirms rig-native-Claude vs needing an SDK-routed path.
- OpenClaw is open-source → its auth code is the concrete recipe (endpoint, headers, token acquisition, any system-prompt requirement) a vendored rig provider must replicate. **Round 6 extracts it.**

**Updated posture:** rig-native-Claude (subscription OAuth) is now the **likely-viable primary**, hybrid demoted to fallback-if-the-mechanics-spike-fails. The remaining work is mechanical (extract OpenClaw's auth contract; then the empirical S1 spike becomes a confirmation, not a coin-flip), not a question of whether Anthropic permits it.


---
---

# Round 6 — auth-contract verdict: token-determined wire, attribution unconfirmed

> Closes the round-5 attribution residual by reading the auth source of the Anthropic-named tools (OpenClaw, Conductor). Verify 8/8. The official @ClaudeDevs tweet (round-5 addendum) confirmed programmatic subscription usage is supported; this round determines token-determined vs sdk-routed.

# Round-6 Final Report — Closing the Attribution-Gate Residual: Token-Determined vs SDK-Routed

**Date:** 2026-06-05 · **Scope:** Resolves the narrow T-vs-S residual left open after the 2026-05-13 @ClaudeDevs tweet, by reading the actual auth source of the Anthropic-named third-party tools (OpenClaw, Conductor). Decision-ready for a Rust engineer who read rounds 1–5.

**What changed since the draft (critique-driven corrections):**
1. Removed the "Bearer confirmed C2" stamp from the §3 recipe — it contradicted this report's own pi-mono#2751 evidence. Transport is now flagged UNRESOLVED with a fallback strategy.
2. Stripped fabricated rig source line numbers (the draft simultaneously guessed the wrong file path); citing by symbol name only.
3. Held the system-prefix and credit-attribution claims at their already-downgraded confidence; §3 tables are now labeled by source quality and do not silently re-inflate.
4. Recipe now mandates pinning a *current* `claude-cli/<ver>` UA and an explicit `anthropic-version`, instead of inheriting OpenClaw's stale `2.1.75`.

---

## 1. Verdict

**Split verdict — and the split is the whole answer:**

| Layer | Verdict | Confidence |
|---|---|---|
| **Wire success** (does a raw rig client get a 200 from `/v1/messages` with a subscription OAuth token?) | **TOKEN-DETERMINED** | **High** — code-confirmed in OpenClaw production source |
| **Credit attribution** (does that 200 draw from the June-15 Agent SDK credit pool vs. metered as overage/blocked?) | **UNCONFIRMED / undocumented** | **Medium-low** — no primary source either way; Anthropic only documents "through the Agent SDK" |

**What the blessed tools actually do (read this session, primary source):**

- **OpenClaw is genuinely raw-API — confirmed against literal source.** `src/llm/providers/anthropic.ts@main` imports `@anthropic-ai/sdk` and uses it as a *plain HTTP client*: on the OAuth path (token detected via `apiKey.includes("sk-ant-oat")`) it builds `new Anthropic({ apiKey: null, authToken: apiKey, baseURL })` and calls `client.messages.create(...)`. With `apiKey: null` + `authToken`, the SDK emits `Authorization: Bearer <token>` and no `x-api-key`. A grep for `spawn|execa|child_process|agent-sdk|/claude` in that file returned **zero matches** — OpenClaw does **not** spawn the `claude` binary and does **not** embed the Agent SDK agent runtime for inference. ([anthropic.ts](https://github.com/openclaw/openclaw/blob/main/src/llm/providers/anthropic.ts))
  - **Contradiction inside OpenClaw's own repo, resolved:** `docs/concepts/oauth.md` says OpenClaw "treats Claude CLI reuse and `claude -p` usage as sanctioned." That is the *credential-acquisition / ToS-posture* story (how it obtains the keychain token and why it believes it is allowed) — **not** the inference transport. The *code* is authoritative over the *docs prose*: inference goes over raw `/v1/messages`. ([oauth.md](https://github.com/openclaw/openclaw/blob/main/docs/concepts/oauth.md))

- **Conductor is SDK/binary-routed and closed-source.** Its FAQ: it "comes bundled with its own installation of Claude Code and Codex" and "uses the auth tokens already saved on your machine" ([conductor.build/docs/faq](https://www.conductor.build/docs/faq)). It spawns the real binary. It cannot be read and demonstrates no raw-API path — it is **sdk-routed (observable)**; it neither supports nor refutes the raw path.

**Consequence for architecture:** The raw rig-native wire works (high confidence). Whether it *attributes to the credit pool* is the single unresolved fact — and **only an empirical S1 spike post-June-15 settles it.** Therefore:

- The build is **GREEN to start**, but **hybrid-capable from day one** is mandatory.
- **Primary until S1 passes: HYBRID** — Claude inference via the official `claude -p` binary in-sandbox (which RightClaw already does), rig owning self-host vLLM + the MCP tool layer.
- **If S1 confirms credit attribution for the raw client → flip Claude to rig-native** as primary, demoting the binary to fallback. Build the vendored rig provider now so the flip is a config change, not a rewrite.

---

## 2. #37205 Reconciliation — does raw-API OAuth work today?

**Yes, the wire works — #37205 was a naive-contract failure, not proof of a block.** Both halves verified against primary sources.

**What #37205 actually was** (verified via GitHub REST API): a feature request asking Anthropic to accept `sk-ant-oat01-*` tokens on the Messages API, **closed `not_planned`, labeled `invalid`/`stale`, auto-closed for inactivity with zero Anthropic-staff engagement** (every comment from `github-actions[bot]` or the reporter, all `author_association=NONE`). It documents the error `"OAuth authentication is currently not supported."` ([claude-code#37205](https://github.com/anthropics/claude-code/issues/37205)). Critically, **#37205 contains no request construction** — no headers, no `anthropic-beta`, no system prompt. It shows only that *some incomplete request* failed. The precise set of omitted elements is therefore inferred, not stated in the issue.

**What actually works** (curl-level proof in [claude-code#40515](https://github.com/anthropics/claude-code/issues/40515)): `POST https://api.anthropic.com/v1/messages` returns **200 for `claude-opus-4-6` and `claude-sonnet-4-6`** when the request carries:
- `Authorization: Bearer <sk-ant-oat token>`
- `anthropic-beta: claude-code-20250219,oauth-2025-04-20`
- `user-agent: claude-cli/<ver> (external, cli)`, `x-app: cli`
- a `system` field whose **first block is exactly** `"You are Claude Code, Anthropic's official CLI for Claude."`

Two distinct gates exist: (a) an identity/transport gate that produces the literal #37205 error string, and (b) an undocumented **non-Haiku system-prefix gate** — Opus/Sonnet return HTTP 400 without the prefix; **Haiku is exempt** (200 either way).

**Reconciliation:** #37205 is **wrong-scope, not stale and not disproving**. The Messages API is **header/identity-gated, not OAuth-blocked**. OpenClaw's production code is the working existence proof.

**Transport remains UNRESOLVED (this is the report's single sharpest open conflict):**
- [claude-code#40515](https://github.com/anthropics/claude-code/issues/40515) and OpenClaw production code: **Bearer works** *with* full identity/beta headers.
- [pi-mono#2751](https://github.com/badlogic/pi-mono/issues/2751) (Apr 2026, patch-verified): the same `sk-ant-oat` token is **rejected via Bearer** with the literal #37205 error string and **works via `x-api-key`**.

Best current reconciliation: the determinant is plausibly **header/identity completeness, not the transport name** — and both the official auth doc and OpenClaw send OAuth via **Bearer**. But this is not settled; a current June-2026 curl in the S1 spike must confirm the live transport. Do not treat Bearer as "confirmed."

---

## 3. The Concrete rig Auth Recipe

**rig template confirmed merged:** rig PR [#1615](https://github.com/0xPlaygrounds/rig/pull/1615) (*"feat(rig-core): add ChatGPT Subscription, GitHub Copilot, and compatibility providers"*) is **merged** (commit `6dc36d8`). The OAuth pattern lives at **`crates/rig-core/src/providers/chatgpt/mod.rs`** (the `crates/`-prefixed path; the bare `rig-core/...` path 404s). The load-bearing *architectural shape* (verified by reading the file's symbols; **exact line numbers not verified — cited by symbol only**):

- **Empty `ApiKey` impl suppresses the default key header:** `impl ApiKey for ChatGPTAuth {}` — the empty body means rig emits no `x-api-key`/default key header.
- **Bearer injection per-request:** `add_auth_headers` sets `Authorization: Bearer {access_token}`.
- **Custom identity headers:** `with_custom` injects `user-agent`, `originator`, accept, etc.
- **Token carried in the auth struct** (`access_token: String`) and read per-request — rotation is a struct swap, not a client rebuild.

This is exactly the shape a vendored **Anthropic-OAuth** provider must take (rig's stock `AnthropicKey` hardwires `x-api-key`, so vendoring is required). Replicate as follows:

**Endpoint:** `POST https://api.anthropic.com/v1/messages` (SDK-default `base_url`).

**Auth (UNRESOLVED — do not stamp "confirmed"):** Send `Authorization: Bearer <access_token>` with an empty `ApiKey` impl so rig emits no `x-api-key`. This matches OpenClaw production + #40515. **But pi-mono#2751 (patch-verified) reports Bearer rejected and `x-api-key` working.** Implementation strategy: **try Bearer first; fall back to `x-api-key` on the #37205 error string; settle definitively with a live curl in S1.** The determinant is plausibly header completeness, not transport name.

**Required headers:**
| Header | Value | Source quality |
|---|---|---|
| `anthropic-beta` | Must include `claude-code-20250219` and `oauth-2025-04-20`. **Treat as a superset, order-varying** — other betas (e.g. `interleaved-thinking-2025-05-14`, prompt-caching) get merged in; it is *not* a fixed two-value pair. | Tool source (OpenClaw + opencode-claude-auth) — primary |
| `anthropic-version` | **Send explicitly. `2023-06-01` is the safe value.** OpenClaw relies on the `@anthropic-ai/sdk` default on the message path and pins `2023-06-01` only on the usage endpoint; a rig client MUST set it itself. **(UNKNOWN: the SDK default for the message path in v0.100.1 — verify in S1.)** | Mixed; pin explicitly |
| `user-agent` | `claude-cli/<ver> (external, cli)`. **Pin a *current* version, keep it updatable** — do NOT inherit OpenClaw's stale `2.1.75`; #40515 used `2.1.85`. **(UNKNOWN: whether Anthropic validates the version string or only the `claude-cli/` prefix — a stale value may eventually be rejected.)** | Tool source — primary; version validation UNKNOWN |
| `x-app` | `cli` | Tool source — primary |
| `anthropic-dangerous-direct-browser-access` | `true` (OpenClaw sets it) | Tool source — primary |

**Required system-prompt prefix (per #40515 — community-verified, NOT Anthropic-documented):** first `system` block exactly `"You are Claude Code, Anthropic's official CLI for Claude."`, then the real prompt. Required for Opus/Sonnet 200; **Haiku exempt.** The *code* (OpenClaw `buildAnthropicSystemBlocks`) always prepends it as the first block on the OAuth path — that much is code-confirmed. The "required for acceptance" portion rests on #40515's reproducible curl table, a community issue, not Anthropic docs. The **minimal accepted form** (first-block-only vs. anywhere in `system`) is **UNKNOWN** (#40515 says first array entry *or* start of the string both work).

**OAuth token detection:** treat as a subscription token if the string contains `sk-ant-oat`.

**Token acquisition — two options:**

*Option A — full Authorization-Code + PKCE (S256), confirmed verbatim against OpenClaw source:*
- `authorize`: `https://claude.ai/oauth/authorize`
- token exchange **and** refresh: `https://platform.claude.com/v1/oauth/token` (`grant_type=authorization_code`, then `refresh_token`)
- `client_id`: `9d1c250a-e61b-44d9-88ed-5944d1962f5e` (fixed public)
- redirect: loopback `http://localhost:53692/callback` (host default `127.0.0.1`, overridable)
- scopes: `org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload`
- refresh `~5 min` before `expires_in`; refresh returns `{access_token, refresh_token, expires_in}`.

*Option B — `claude setup-token` (simpler; what RightClaw already does):* mints a **1-year subscription OAuth token** (`CLAUDE_CODE_OAUTH_TOKEN`), inference-scoped, per the [official auth doc](https://code.claude.com/docs/en/authentication). The rig provider can consume this token directly and skip the PKCE dance, refreshing only if Anthropic shortens the token lifetime.

**Credit-usage verification endpoint (the S1 measurement instrument, from OpenClaw):** `GET https://api.anthropic.com/api/oauth/usage` with `Authorization: Bearer <token>` + `anthropic-beta: oauth-2025-04-20`. This is how OpenClaw reads the credit ledger.

*(Sources: [openclaw anthropic.ts](https://github.com/openclaw/openclaw/blob/main/src/llm/providers/anthropic.ts), [openclaw oauth/anthropic.ts](https://github.com/openclaw/openclaw/blob/main/src/llm/utils/oauth/anthropic.ts), [openclaw provider-usage.fetch.claude.ts](https://github.com/openclaw/openclaw/blob/main/src/infra/provider-usage.fetch.claude.ts), [opencode-claude-auth src/index.ts](https://github.com/griffinmartin/opencode-claude-auth/blob/main/src/index.ts) (independent header corroboration), [claude-code#40515](https://github.com/anthropics/claude-code/issues/40515), [pi-mono#2751](https://github.com/badlogic/pi-mono/issues/2751), [rig chatgpt/mod.rs](https://github.com/0xPlaygrounds/rig/blob/main/crates/rig-core/src/providers/chatgpt/mod.rs), [official auth doc](https://code.claude.com/docs/en/authentication).)*

---

## 4. ToS Read

**Headline: Anthropic blesses the programmatic *envelope* (subscription token → credit pool), but has never blessed identity-header *spoofing* from a non-Anthropic binary.** That is the precise residual exposure.

- **Blessed, primary:** The @ClaudeDevs tweet ([x.com/ClaudeDevs/…](https://x.com/ClaudeDevs/status/2054610152817619388)) names OpenClaw and Conductor as supported and says they "draw from your credit **the same way your own scripts do**." The support article ([support.claude.com/…/15036540](https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan)) confirms the credit covers Agent SDK, `claude -p`, GitHub Actions, and third-party apps. Programmatic subscription usage is **not** blocked or first-party-only. Round 5's "is this even allowed" fear is **deflated**.

- **The residual, sharply scoped:** Every *official* phrasing restricts third-party coverage to apps authenticating **"through the Agent SDK"** (support article, verbatim). Anthropic **never documents attribution as token-determined**, and **never blesses a hand-rolled raw client that spoofs `user-agent: claude-cli` + the "You are Claude Code" system prefix**. The two named tools sidestep this in opposite ways and neither is a clean precedent for "non-Anthropic binary impersonating claude-cli over raw HTTP": **Conductor runs the real binary; OpenClaw's *prose* leans on CLI-reuse as the sanctioned story even though its *code* is raw-API.**

- **Honest read:** Replicating the *blessed programmatic envelope* (subscription token → credit pool) is within ToS. Whether **spoofing the claude-cli identity contract from a custom Rust client** is within ToS is **UNKNOWN** — no primary source addresses it. The identity headers + system prefix are a **technical gate**, not a documented license grant. Reduced risk, **not eliminated**.

- **Mitigation:** The hybrid (spawn the real `claude -p`) is **unambiguously within ToS** — it *is* the binary Anthropic ships. Keep it the attribution-safe default. The rig-native raw path is an opt-in to validate, not the safe assumption.

---

## 5. Updated Go/No-Go

**Build status: GREEN to start. Architecture: HYBRID-primary now, with the vendored rig provider built in parallel and gated behind S1.**

**What the S1 empirical spike must confirm post-June-15 (the sole remaining gate):**
1. **Credit attribution (the crux):** Make one raw `/v1/messages` call with the full contract (OAuth token + `anthropic-beta: claude-code-20250219,oauth-2025-04-20` + current `claude-cli` UA + `x-app: cli` + "You are Claude Code" prefix), then `GET /api/oauth/usage`. **Did the Agent SDK credit decrement?** Yes → token-determined attribution → rig-native primary. 4xx or unchanged credit → SDK-routed → hybrid stays primary. *(No primary source answers this; undocumented either way.)*
2. **Transport (Bearer vs `x-api-key`):** resolve the #40515-vs-pi-mono#2751 conflict with a current curl.
3. **Tool-use billing lane (Hermes #15080 retest):** `tools=True` previously returned 400 "out of extra usage" for Max-only accounts ([hermes-agent#15080](https://github.com/NousResearch/hermes-agent/issues/15080)) — a billing-lane gap, not an auth-mechanism gap. Confirm the June-15 credit pool fills that lane. *(Inferred-resolved, NOT confirmed by a post-June-15 test.)*
4. **`anthropic-version` SDK default** for the message path, and whether the `claude-cli/<ver>` string is version-validated.
5. **Non-spoofable client attestation:** verify the SDK/binary attaches no signed attestation or baked-in distinct `client_id` that a rig client can't replicate. If it does, wire success ≠ attribution.

**Final architecture per verdict:**

> **Now (until S1):** **Hybrid.** Claude inference via the official `claude -p` binary in-sandbox (current RightClaw behavior — ToS-clean, attribution-safe). rig owns self-host vLLM providers + the MCP tool layer. Build the vendored rig Anthropic-OAuth provider (§3) in parallel, behind a flag.
>
> **After S1 passes (credit decrements for the raw client):** flip Claude to **rig-native dual-provider** — vendored Anthropic-OAuth provider as primary, `claude -p` binary demoted to fallback. The vLLM API-key path stays as the always-safe escape hatch.

**Do not overclaim:** The wire is token-determined (high confidence, code-confirmed). **Attribution is not confirmed token-determined** — Anthropic only documents "through the Agent SDK," and the one observable named tool (Conductor) is binary-routed. Keep hybrid primary until the credit ledger says otherwise.

---

## Concrete next action

**Build the vendored rig Anthropic-OAuth provider now, behind a feature flag, modeled on `crates/rig-core/src/providers/chatgpt/mod.rs` (empty `ApiKey` impl + per-request `add_auth_headers` Bearer + `with_custom` identity headers), consuming the existing `CLAUDE_CODE_OAUTH_TOKEN`.** Then, **on/after June 15, run the S1 spike as the first thing the provider does in a test harness:** one raw `/v1/messages` call with the full identity contract → `GET /api/oauth/usage`, asserting the Agent SDK credit decremented, and a paired Bearer-vs-`x-api-key` curl to settle transport. The spike result is the single switch that flips Claude from hybrid (binary) to rig-native primary. Until it passes green, ship hybrid.

---

**Key files / sources for the implementer:**
- rig template: `crates/rig-core/src/providers/chatgpt/mod.rs` — `impl ApiKey for ChatGPTAuth {}` (suppresses default key header), `add_auth_headers` (Bearer), `with_custom` (custom identity headers). *(symbols verified; line numbers not verified.)*
- OpenClaw auth: `src/llm/providers/anthropic.ts` (wire contract + identity headers + system prefix), `src/llm/utils/oauth/anthropic.ts` (PKCE flow values), `src/infra/provider-usage.fetch.claude.ts` (credit-ledger endpoint), `docs/concepts/oauth.md` (ToS posture — prose, not transport).
- Gates/reconciliation: claude-code#40515 (working contract + curl), #37205 (the naive failure), pi-mono#2751 (transport conflict — UNRESOLVED), hermes-agent#15080 (tool-use billing lane — inferred-resolved).
- Anthropic primary: @ClaudeDevs tweet, support article 15036540, code.claude.com/docs/en/authentication.

**Honestly flagged as unverified:** exact rig source line numbers; the live June-2026 transport (Bearer vs `x-api-key`); the *minimal* accepted form of the system prefix; whether the SDK/binary attaches a non-spoofable attestation; the post-June-15 Hermes #15080 retest; and the crux — whether a raw rig client's 200 decrements the Agent SDK credit. All but the line numbers are S1 gates.


---
---

> **Reliability note (round 7):** verify phase mostly failed (3/12 fact-checkers returned structured output — large-run flakiness); the report's confidence comes from the synthesis being grounded in `main`-branch source with verbatim quotes + the critique pass, not independent adversarial verification. Source-cited claims are solid; treat any uncited inference as lower-confidence.

# Round 7 — MiMo-Code under the RightClaw lens

*Decision-ready for a Rust engineer who read rounds 1–6. Harness decision (rig) is settled; this round asks whether MiMo-Code reopens it, contributes a model provider, or contributes reusable ideas. Short answer: ideas + a self-host-only-realistic provider, excluded as a base by the closed-box threat model. **rig stays.***

All claims carry a primary-source URL on the `main` branch and a verbatim quote. Where the original draft cited the dead `dev` branch or paraphrased quotes, those are corrected or marked. Origin tags distinguish **MiMo-added** / **inherited-from-opencode** / **ACP-protocol** / **model-provider**.

---

## 1. MiMo-Code architecture & the opencode delta

**Fork depth.** `packages/opencode/` is the opencode CLI source renamed in-place to the MiMo CLI package (`package.json`: `"name": "@mimo-ai/cli"`, `"bin": {"mimo": ...}`), vendoring opencode's entire dependency tree and `src/` with **no `opencode` npm dependency** — an in-place core fork, not a wrapper. It inherits opencode wholesale, including a verbatim `SECURITY.md` whose body still names OpenCode ("OpenCode does **not** sandbox the agent. The permission system exists as a UX feature…") and points to `security@anoma.ly`.
*Correction vs draft:* that file is titled `# Security`, not "OpenCode" — only the body bears OpenCode branding. The draft's `dev`-branch URL 404s; evidence is on `main`. [SECURITY.md (main)](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/SECURITY.md)

**What MiMo adds inside the core** (all confirmed against `main`):
- **Persistent memory** — Markdown file tree (`memory/{global,projects,sessions}/*.md`) indexed by **SQLite FTS5 / BM25** (`memory/fts.sql.ts`, `memory/service.ts`). Not a vector store, not an external API.
- **Checkpoint-based context management** — background checkpoint-writer subagents fire at token thresholds and fill a fixed 11-section `checkpoint.md`; on overflow the loop inserts a rebuild boundary marker (**never deletes DB rows**) and rebuilds from checkpoint + memory. LLM-summary compaction is only the no-checkpoint fallback (`session/prompt.ts`, `prune.ts`).
- **dream/distill self-improvement** — two autonomous maintenance agents on separate cadences (`auto-dream.ts`).
- **Subagent orchestration** + a superpowers-skills "compose" port.

**Transport.** The TUI↔core path is opencode's **HTTP server + typed SDK**, *not* ACP. ACP is an opt-in external-client mode (`mimocode acp`) whose adapter literally spins up the HTTP server and wraps the SDK (`createOpencodeClient`). This is load-bearing for the §4 verdict. [acp.ts (main)](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/cli/cmd/acp.ts)

---

## 2. dream/distill + memory + context management — reusable patterns

All three subsystems are **agentic** (constrained subagents driven by long prompts, fire-and-forget), not deterministic pipelines — the opposite of RightClaw's deterministic prefilter→probe-writer→curator with explicit gates and a spend ledger. That trade (model-driven flexibility vs determinism/cost-control) is the headline tension when borrowing.

**Memory.** Markdown-as-index over an authoritative read-only trajectory DB (`session/message/part` tables, JSON-in-TEXT). The curator invariant is explicit: *"Raw trajectory is authoritative; memory files are a structured index/cache."* Search is OR-joined FTS5 ranked by BM25 with a **relative** score floor (default 0.15× top hit, #1 always kept, 3× over-fetch) — chosen because absolute BM25 thresholds break on small corpora. [service.ts (main)](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/memory/service.ts)

**dream** (~7d): consolidates memory, verifying candidate facts against the read-only trajectory DB via bash+SQLite, verifying paths with Glob / symbols with Grep, marking unverifiable claims `[unverified]`, keeping `MEMORY.md` under 200 lines/10KB. Promote only on *"an explicit user statement, a clear design decision, or repeated evidence across sessions."* (Verbatim-confirmed; `/dream` is a manual command, framed here as a subagent.) [dream.txt (main)](https://raw.githubusercontent.com/XiaomiMiMo/MiMo-Code/main/packages/opencode/src/agent/prompt/dream.txt)

**distill** (~30d): finds workflows recurring **≥2×**, **inventories existing skills first to avoid duplication**, then writes ClawHub-compatible `SKILL.md` (YAML `name`+`description`). [distill.txt (main)](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/agent/prompt/distill.txt)

**Scheduling**: no built-in scheduler — fired from the loop on the first step of a top-level session, interval-gated via a marker row, with project-too-young and 10s-debounce guards.

### Genuinely reusable (ranked)

1. **Checkpoint-over-summary compaction (high — but with a cost caveat).** The pattern — background structured-checkpoint write at graduated thresholds, then rebuild-from-checkpoint on overflow instead of a lossy summarization turn — is liftable to the rig worker loop, which owns the message vector and can rebuild deterministically. The fixed 11-section template, per-section token budgets, and the **"preserve EXACT-FORM literals byte-for-byte"** rule port directly. **Caveat (critique fix #5):** MiMo's checkpoint-writer forks at a watermark "for prefix-cache alignment," and that cost advantage depends on **Anthropic-family prefix caching** — which `supportsCacheMarkers` denies to non-Claude self-host backends (§3). On a non-caching backend the rebuild re-sends a large prefix every overflow with **no cache discount**. The *pattern* stands on engineering merit (it avoids a lossy summary turn); its *cost story* does not survive a non-Claude backend. Adopt the structure; do not assume the savings. This recommendation does **not** rely on MiMo's self-reported 200+-step benchmark (critique fix #4) — the checkpoint structure is independently justified.
2. **Trajectory-as-truth + memory-as-index, with a verification gate (high).** RightClaw already archives Telegram messages in `data.db` (Turso FTS); let the curator **verify** candidate skills/memories against that durable transcript (and the live filesystem) before persisting, tagging unverifiable as `[unverified]`.
3. **Memory promotion ladder (high).** session-checkpoint → project `MEMORY.md` → global, each gated on "proven cross-session-durable." Maps to a graded RightClaw curator lifecycle (per-thread/ephemeral → per-agent durable → cross-agent) with a verification gate at each step — better than binary retain/skip.
4. **distill = anti-duplication skill synthesis (high).** Inventory existing skills before writing; RightClaw's prefilter already reads the sandbox skill index (`rightx-*`), so this is cheap.
5. **Graduated pressure nudge (med).** 0.50/0.70/0.85 ladder injecting a synthetic "context filling up — flush learnings now" reminder; a zero-extra-invocation complement to Hindsight auto-retain.
6. **Relative BM25 floor for FTS recall (med).** Lift into right-db's Turso FTS query builder — RightClaw's transcript search has the identical small-corpus problem.
7. **System-spawned subagent isolation (med).** Maintenance turns run deny-by-default, tight tool allowlist, read-only DB. Mirror this for any rig maintenance turn.

---

## 3. Did MiMo fix opencode's round-3 gaps? + MiMo as provider

**Confirmed against `main` source — the fix-or-inherit triad is source-backed, not opencode-inferred:**

| Round-3 gap | MiMo status | Evidence |
|---|---|---|
| **Structured output emulated** | **Still emulated, but hardened.** Synthetic `StructuredOutput` tool registered on `format.type === "json_schema"`, `toolChoice:"required"`, bounded repair-retry (`retryCount` default 2), typed `StructuredOutputError` on exhaustion. **Stronger than round-3 opencode** (which lacked the repair loop). | [prompt.ts](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/prompt.ts), [message-v2.ts](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/session/message-v2.ts) |
| **SDK omitted `format`** | **Fixed.** Typed/generated SDK + OpenAPI now expose `format?: OutputFormat` on `session.prompt`/`prompt_async` — no longer raw-HTTP-only. | [sdk.gen.ts](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/sdk/js/src/v2/gen/sdk.gen.ts) |
| **Caching gated to Anthropic-family** | **Inherited & widened, still Claude-only.** `supportsCacheMarkers()` returns false for generic `@ai-sdk/openai-compatible` unless the id matches claude/anthropic; widened to Bedrock/OpenRouter/Copilot/Alibaba Claude variants. A non-Claude self-host gets no inline cache markers. | [transform.ts](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/provider/transform.ts) |
| **Skills `allowed-tools` ignored** | **Still silently ignored.** Frontmatter parsed via `Info.pick({name, description, hidden})` — no allowed-tools field read. | [skill/index.ts:161](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/skill/index.ts) |
| MCP client | Full & usable (stdio + StreamableHTTP + SSE + OAuth). | [mcp/index.ts](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/mcp/index.ts) |

Native AI-SDK `generateObject`/`streamObject` is used only **internally** for the goal-loop "Verdict" judge, not the user-facing prompt path (`session/goal.ts`).

### MiMo models as a provider

**Weights are open and self-hostable** — verified, not assumed: MiMo-V2.5-Pro is **MIT** on HuggingFace + ModelScope, safetensors F8_E4M3, with official **vLLM and SGLang** cookbooks; 1.02T total / **42B active** MoE, 1M context. [HF model card](https://huggingface.co/XiaomiMiMo/MiMo-V2.5-Pro)

**Two material caveats:**
- **Multi-turn tool-call 400 (OPEN, unresolved).** [Issue #44](https://github.com/XiaomiMiMo/MiMo/issues/44) (filed 2026-05-17, no maintainer fix): the OpenAI-compatible endpoint rejects multi-turn tool-call history with `400 Param Incorrect`; reasoning-enabled multi-turn requires persisting `reasoning_content` across turns. A real risk for an agentic loop. Self-hosting via vLLM/SGLang's own OpenAI server likely sidesteps the hosted gateway's validator, but this is **untested** for rig.
- **Hosted gateway moderation/egress.** `error.ts` maps HTTP 421 (content moderation) / 441 (risk control) — a Chinese-lab hosted-API data-egress surface.

**Self-host footprint reality (critique fix #3).** A 1.02T-param MoE at FP8 is **≈1 TB of weights** — a multi-node H100/H200 cluster, **categorically not a per-agent OpenShell sandbox or a modest host**. So:
- **Hosted-API-only is the realistic access path**, and it is excluded by the closed-box threat model.
- **Self-host does NOT cheaply rescue the egress objection** — it requires datacenter-class hardware. The earlier framing of "self-host MiMo as a per-agent provider profile" is downgraded: viable only as a shared, operator-run, datacenter-class endpoint, not as sandbox-local inference.

The structured-output enforcement *pattern* (synthetic tool + `toolChoice:required` + bounded repair + typed error) remains the most reusable provider-layer takeaway — it is exactly the "model must honor the lever" fallback rig needs for non-Anthropic backends, independent of MiMo the model.

---

## 4. ACP control surface — RightClaw as client, MiMo/opencode as agent

MiMo ships a near-complete ACP agent adapter (`src/acp/agent.ts`, ~1783 LoC, ACP SDK 0.16.1). **The in-repo `acp/README.md` is stale** ("no streaming, no tool-call reporting" — all actually implemented). Grep the source, never the feature doc.

**What an ACP client CAN do:** observe every tool call (`tool_call` pending + `tool_call_update` with `rawInput`/`rawOutput`); stream `agent_message_chunk` / `agent_thought_chunk` (reasoning) / `usage_update`; create/load/**fork** (`unstable_forkSession`)/resume/list sessions; **set model per session** (`unstable_setSessionModel`); set mode/config; cancel mid-turn; serve fs/terminal back to the agent. This **reconciles part of round 2** — forking and per-session model selection now exist (as `unstable_` methods, provisional in SDK 0.16.1).

**What an ACP client CANNOT do (round-2 gaps persist, now confirmed agent-side):**
- **Modify tool input.** `session/request_permission` returns only `cancelled` or `selected{optionId}` (allow_once / allow_always / reject_once / reject_always — confirmed against the [ACP tool-calls schema](https://agentclientprotocol.com/protocol/tool-calls)). No modify-input kind exists. MiMo proves it: a "modified edit" approval is applied by re-writing the file via the client's own `fs/writeTextFile` from the diff — a **side-channel write**, not feeding altered input into the agent's tool call.
- **Enforce structured output.** `session/prompt` returns only `{ stopReason, usage, _meta:{} }`. No json-schema/responseFormat/outputSchema anywhere in `src/acp/`. `_meta` *is* used — but only for model-variant pinning (`_meta.opencode.variant`) and terminal-auth, **not** structured output or loop control.

**ACP vs HTTP/SDK (critical).** Driving MiMo over ACP gives **strictly less** control than the HTTP server + `@mimo-ai/sdk`, because the ACP adapter literally spins up that server and calls the SDK. Anything ACP does, the SDK does directly — plus things ACP flattens away (raw `permission.reply` semantics, `mcp.add`, message replay). ACP's only adds are editor-standard stdio JSON-RPC framing (Zed compatibility) and the permission UX round-trip — neither of which RightClaw needs.

**Verdict:** Over ACP, RightClaw-as-client would **observe-and-gate but never own the loop, never enforce structured output, never modify a tool call inline.** ACP gets nowhere near owning the loop. **rig (which owns the message vector and the loop) remains correct.** The reconciliation with round 2: ACP *evolved* (forking + model selection landed), but the two load-bearing gaps — no modified-input, no structured-output round-trip — are unchanged.

---

## 5. License / Use-Restrictions / ToS / security — and the RightClaw verdict

**License (clean).** Source is **standard MIT**, dual copyright (Xiaomi 2026 + opencode 2025), no clauses beyond the MIT template. [LICENSE](https://raw.githubusercontent.com/XiaomiMiMo/MiMo-Code/main/LICENSE)

**USE_RESTRICTIONS.md** is acceptable-use/ethics only (no unlawful/IP-infringing/harassing/**military**/malicious-cyber/unauthorized-data/unsupervised-high-risk use). **No commercial-use ban, no competing-service ban, no geographic limit beyond military.** So the *code patterns* are freely borrowable. [USE_RESTRICTIONS.md](https://raw.githubusercontent.com/XiaomiMiMo/MiMo-Code/main/USE_RESTRICTIONS.md)

**Hosted ToS** is operated by **Xiaomi Technologies Singapore Pte. Ltd.** (non-PRC), Xiaomi as Processor / user as Controller, selectable storage regions — but a Chinese-lab service. **UNKNOWN (critique fix #6, decision-relevant — do not drop):** the exact training-data clause (whether Xiaomi trains on hosted-API inputs/outputs, opt-out, retention) could not be captured — the user-agreement page is JS-rendered and resisted fetching. This is the single most decision-relevant fact for any hosted-API egress decision; it remains **unverified**.

**The strongest security catch (critique fix #7 — promoted to the verdict).** The MiMo-added free provider `plugin/mimo-free.ts` computes a **device fingerprint = sha256(hostname|platform|arch|cpu-model|username)**, POSTs it to `api.xiaomimimo.com/api/free-ai/bootstrap` to mint an anonymous JWT, and routes **all** chat to Xiaomi's hosted endpoint **by default** (`apiKey:"anonymous"`); the MiMo websearch tool defaults to the same host. This is a concrete, default-on anti-pattern **directly opposed to RightClaw's credential-isolation invariant** — and the cleanest single reason MiMo-Code-as-base is excluded. [mimo-free.ts](https://github.com/XiaomiMiMo/MiMo-Code/blob/main/packages/opencode/src/plugin/mimo-free.ts)

**No covert runtime telemetry in core** — the only PostHog code is a maintainer-only download-stats script (`script/stats.ts`, gated on `POSTHOG_KEY`, not wired to runtime); OpenTelemetry is opt-in (default off). SECURITY.md inherited verbatim ("does not sandbox the agent").

---

## Verdict

**rig stays the harness. MiMo-Code does not reopen opencode-as-base.** As a base it (a) inherits opencode's round-3 gaps (emulated structured output, Claude-only prompt caching, ignored skills `allowed-tools`), (b) is **unsandboxed by design**, and (c) ships a **default Chinese-hosted egress path that fingerprints the host** — the exact opposite of RightClaw's closed-box, credential-isolated posture. Over ACP, a client can observe-and-gate but **cannot own the loop, modify tool input, or enforce structured output** — strictly less than rig.

MiMo-Code is valuable as **(1) an ideas source** (dream/distill, checkpoint-over-summary compaction, memory promotion ladder, relative BM25 floor — all MIT, freely borrowable into the rig build) and **(2) a provider only in the narrow, qualified sense** that MiMo-V2.5-Pro weights are MIT — but at ~1 TB/1.02T-param it is **datacenter-class, hosted-API-realistic only**, and the hosted API is excluded by the threat model. **Self-host does not cheaply escape the egress objection.** The reusable provider-layer artifact is the structured-output enforcement *pattern*, not the model.

Nothing here overturns rounds 1–6.

## Ranked action list

1. **Lift the structured-output enforcement pattern** into rig's non-Anthropic fallback: synthetic StructuredOutput tool + `toolChoice:"required"` + bounded repair-retry (default 2) + typed error. Two-tier with rig-native `generateObject` for trusted internal judgments. *(Highest-value, lowest-risk takeaway.)*
2. **Adopt checkpoint-over-summary compaction** in the rig worker loop (11-section template, per-section budgets, byte-for-byte literal rule, rebuild-from-checkpoint). **Annotate the cost caveat:** no prefix-cache discount on non-Claude backends.
3. **Add a verification gate to RightClaw's curator** — verify candidate skills/memories against the durable Turso transcript before persisting; tag unverifiable `[unverified]`. Borrow distill's inventory-before-write anti-duplication.
4. **Add a graded memory promotion ladder** (ephemeral → per-agent durable → cross-agent) with a verification gate per tier.
5. **Lift the relative BM25 score floor** into right-db's Turso FTS query builder.
6. **Add a guard assertion** that no default RightClaw provider ever resolves to a third-party hosted endpoint without an explicit operator-configured credential through the gateway — `mimo-free.ts` is the named anti-pattern.
7. **Defer MiMo-V2.5-Pro** to an optional, operator-run, datacenter-class shared OpenAI-compatible endpoint *if and only if* a cheap-long-context lever is ever needed; gate on a rig spike covering (a) Issue #44 multi-turn tool-call survival and (b) `reasoning_content` round-trip in rig's history layer. **Do not** treat it as sandbox-local or as an egress escape hatch.
8. **Capture the hosted ToS training-data clause** (JS-capable fetch) before any contact with the hosted API — currently UNKNOWN and decision-relevant.

*Self-reported MiMo benchmarks (200+-step superiority) are treated as unverified throughout; no recommendation above depends on them.*


---
---

# Round 8 — MiMo-Code / opencode provider management: programmatic surface

## 1. Direct answer

There is a **partial** programmatic interface, not a full provider-lifecycle control plane. The honest verdict: **credential management has a real runtime HTTP write-API (`PUT/DELETE /auth/:providerID`), but provider *definition* (base_url, headers, model list, adapter) is file/plugin-based — read-only over HTTP.** You can fully configure a custom provider + inject its credential **headlessly with zero TUI**, but the path is hybrid: write the provider *shape* into `opencode.json` (or a plugin `config` hook), and supply the *credential* via any of three headless channels (`PUT /auth`, the `OPENCODE_AUTH_CONTENT` env, or `{env:VAR}`/`{file:path}` substitution inside the config). The interactive `auth login` / `/login` TUI is one option, never the only one. MiMo-Code adds **no new routes** — it inherits opencode's entire server surface and only adds a built-in `MimoFreeAuthPlugin` (the `mimo serve` env override is renamed `MIMOCODE_AUTH_CONTENT`).

## 2. HTTP server (`mimo serve` / `opencode serve`) surface

| Method | Path | What it does | R/W | Origin |
|---|---|---|---|---|
| GET | `/config` | Returns resolved config (incl. provider blocks) | read | opencode-inherited |
| PATCH | `/config` | `config.update` — validates body against `Config.Info`/`ConfigV1.Info`, calls `cfg.update(config)`. Body's `provider` field is a `Record<id, ConfigProvider.Info>` (apiKey/baseURL under `options`, `models` map, per-model `headers`) | **write** (see UNKNOWN) | opencode-inherited |
| PATCH | `/global/config` | `global.config.update` — same, calls `cfg.updateGlobal(config)` | **write** | opencode-inherited |
| GET | `/config/providers` | Lists configured providers + default models (`Provider.ConfigProvidersResult`) | read | opencode-inherited |
| GET | `/provider` | Lists models.dev catalog + connected providers via `ModelsDev.get()` + `svc.list()` | read | opencode-inherited |
| GET | `/provider/auth` | Lists available auth methods per provider | read | opencode-inherited |
| POST | `/provider/:id/oauth/authorize` | Begins provider OAuth dance | write (ends in `auth.set`) | opencode-inherited |
| POST | `/provider/:id/oauth/callback` | Completes OAuth dance | write (ends in `auth.set`) | opencode-inherited |
| **PUT** | **`/auth/:providerID`** | **`auth.set` — writes credentials (`Auth.Info`) to `auth.json` @ 0600. Headless, no TUI.** | **write** | opencode-inherited |
| **DELETE** | **`/auth/:providerID`** | **`auth.remove` — deletes the provider's credential entry** | **write** | opencode-inherited |
| POST | `/session/:id/prompt` | Per-turn model selection: payload carries `providerID` + `modelID` | write (request-scoped) | opencode-inherited |

Sources: control routes — [`packages/opencode/src/server/routes/instance/httpapi/groups/control.ts`](https://github.com/sst/opencode/blob/dbbe67f066fef47761c637624a34b2350cb109c0/packages/opencode/src/server/routes/instance/httpapi/groups/control.ts#L39) (`HttpApiEndpoint.put("authSet", ControlPaths.auth, { payload: Auth.Info })`, `ControlPaths.auth = "/auth/:providerID"`); MiMo Hono equivalent — [`MiMo-Code .../routes/control/index.ts`](https://github.com/XiaomiMiMo/MiMo-Code/blob/HEAD/packages/opencode/src/server/routes/control/index.ts) (`.put("/auth/:providerID", { operationId: "auth.set" }, validator("json", Auth.Info.zod), ...)`). Config — [`.../groups/config.ts`](https://github.com/sst/opencode/blob/dbbe67f066fef47761c637624a34b2350cb109c0/packages/opencode/src/server/routes/instance/httpapi/groups/config.ts#L16) (`patch("update", root, { payload: ConfigV1.Info })`, `get("providers", ...)`). Provider list — [`.../routes/instance/provider.ts`](https://raw.githubusercontent.com/XiaomiMiMo/MiMo-Code/main/packages/opencode/src/server/routes/instance/provider.ts). Prompt — [`.../groups/session.ts`](https://github.com/sst/opencode/blob/dbbe67f066fef47761c637624a34b2350cb109c0/packages/opencode/src/server/routes/instance/httpapi/groups/session.ts#L316).

**There is NO route to register a new provider's transport shape** (base_url/headers/models/npm adapter) — only `PATCH /config` can carry a provider block, and whether it persists to disk is UNKNOWN (see §4).

**No global "set default model" route exists.** Model is selected per-request as `providerID` + `"/"` + `modelID` in the prompt payload — confirmed verbatim in the session route.

## 3. SDK surface (`@opencode-ai/sdk` / `@mimo-ai/sdk`)

Generated client methods (from [`packages/sdk/js/src/v2/gen/sdk.gen.ts`](https://github.com/sst/opencode/blob/dbbe67f066fef47761c637624a34b2350cb109c0/packages/sdk/js/src/v2/gen/sdk.gen.ts#L435)):

```ts
// auth — credential write/remove (headless)
client.auth.set({ providerID: string, auth?: Auth })   // -> PUT /auth/{providerID}
client.auth.remove({ providerID: string })             // -> DELETE /auth/{providerID}

// config — introspection + update
client.config.get(...)            // -> GET /config
client.config.update(...)         // -> PATCH /config   (payload ConfigV1.Info)
client.config.providers(...)      // -> GET /config/providers

// provider — read-only
client.provider.list(...)         // -> GET /provider

// model selection — per-request, via the prompt call
client.session.prompt({ sessionID, providerID, modelID, ... })  // -> POST /session/{id}/prompt
```

`auth` param type is the discriminated union (`auth/index.ts` @ dbbe67f):
```ts
Api       = { type: "api",       key: string, metadata?: Record<string,string> }
Oauth     = { type: "oauth",     refresh, access, expires, accountId?, enterpriseUrl? }
WellKnown = { type: "wellknown", key, token }
Info      = Union([Oauth, Api, WellKnown]) // discriminator: "type"
```
A static API key is injectable as `{ type: "api", key: "sk-..." }` — no OAuth flow required. MiMo's SDK package is `@mimo-ai/sdk`; method shapes match (shared lineage).

## 4. Headless provider + credential recipe (no TUI)

Goal: add a custom OpenAI-compatible provider, inject its key, select the model per turn — fully scripted.

**Step 1 — Define the provider shape (file-based; this is the de-facto provider-definition API).** Write `opencode.json` ([opencode.ai/docs/providers](https://opencode.ai/docs/providers/)):
```json
{
  "provider": {
    "myprovider": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "My Provider",
      "options": { "baseURL": "https://api.example.com/v1", "headers": { "X-Custom": "v" } },
      "models": { "model-id": { "name": "Model Name" } }
    }
  }
}
```
Model metadata not in `models{}` is sourced from the models.dev catalog. **Not interactive.**

**Step 2 — Inject the credential.** Pick ONE (all headless):
- **(a) HTTP/SDK:** `PUT /auth/myprovider` with body `{ "type": "api", "key": "sk-..." }`, or `client.auth.set({ providerID: "myprovider", auth: { type: "api", key: "sk-..." } })`. Writes `auth.json` @ 0600.
- **(b) Env (whole auth.json):** set `OPENCODE_AUTH_CONTENT` (MiMo: `MIMOCODE_AUTH_CONTENT`) to the JSON credential map. Read by `auth/index.ts` before the file fallback ([auth/index.ts#L10](https://github.com/sst/opencode/blob/dbbe67f066fef47761c637624a34b2350cb109c0/packages/opencode/src/auth/index.ts#L10)).
- **(c) In-config substitution:** put `"options": { "apiKey": "{env:MY_KEY}" }` or `{file:/path}` in `opencode.json` — resolved from `process.env` at load, bypassing `auth.json` entirely ([config/variable.ts#L34](https://github.com/sst/opencode/blob/dbbe67f066fef47761c637624a34b2350cb109c0/packages/opencode/src/config/variable.ts#L34)).

**Step 3 — Select the model per turn.** `POST /session/:id/prompt` with `providerID: "myprovider"`, `modelID: "model-id"` (or the SDK prompt call). No global default-model write needed.

**No step is interactive-only.** The TUI `auth login` is merely a human-friendly wrapper over Step 2(a).

UNKNOWN / flags:
- Whether `PATCH /config` *persists* an added provider block to disk (survives restart) vs in-memory/session-scoped — handler write/merge behavior not read. If it persists, Step 1 could move to HTTP. **Do not assume.**
- `PUT /auth` sits behind the server's `Authorization` middleware — a headless client needs the server token; its provisioning mechanism was not traced.
- The `mimo-free` anonymous-JWT bootstrap endpoint plugin source beyond the `config` hook was located but the JWT-mint internals weren't fully read.

## 5. MiMo-added surface

MiMo-Code adds **no provider/auth HTTP routes** beyond opencode. Its only addition is the built-in `MimoFreeAuthPlugin` ([`packages/opencode/src/plugin/mimo-free.ts`](https://github.com/XiaomiMiMo/MiMo-Code/blob/42e7da3d51dba1129cd3abfa214e29f7385924a3/packages/opencode/src/plugin/mimo-free.ts#L122)), which uses the **standard plugin `config(input)` hook** ([`packages/plugin/src/index.ts#L304`](https://github.com/XiaomiMiMo/MiMo-Code/blob/42e7da3d51dba1129cd3abfa214e29f7385924a3/packages/plugin/src/index.ts#L304): `config?: (input: Config) => Promise<void>`) to inject `provider.mimo` (`@ai-sdk/openai-compatible`, baseURL `api.xiaomimimo.com`) with a `wrappedFetch` that mints an anonymous JWT, and disables the upstream `opencode` providers. This is itself the cleanest demonstration of the **plugin-based programmatic provider-definition path** any user can use — an in-process alternative to editing `opencode.json`.

## 6. Verdict & RightClaw note

**Name it plainly:** opencode/MiMo do **NOT** offer a daemon REST control plane for the full provider lifecycle. The HTTP API is (a) **read-only provider/config introspection** (`GET /provider`, `/config`, `/config/providers`), (b) a **credential read/write/delete control plane** (`PUT`/`DELETE /auth/:providerID`), and (c) the OAuth dance. Defining a provider's transport (base_url/headers/models/adapter) is **file-based** (`opencode.json`) or **in-process plugin `config` hook**. Effectively: **file-config-as-provider-definition + read-only introspection HTTP + a real credential control plane + three headless credential channels.** Model selection is per-request, not a global setting.

The short version for the operator: *credentials have a genuine runtime write-API; provider definitions are config-file/plugin only (no runtime write route, persistence of `PATCH /config` provider blocks UNKNOWN); everything is doable headlessly without the interactive CLI.*

**RightClaw relevance (rig + host gateway):** Only one pattern transfers — the **credential/definition split**. opencode keeps the credential write-path (`auth.set`/`auth.json`/env) separate from the provider transport definition (config/plugin), and offers **three independent headless credential channels** so a key never has to touch the interactive TUI. RightClaw already enforces the harder version of this boundary (gateway holds credential bytes, never `agent.yaml`/backups/logs; placeholder substitution at the proxy), so opencode's design is a weaker precedent, not a model to adopt. The one concretely useful takeaway: opencode's `{env:VAR}`/`{file:path}` config substitution is the same headless-injection ergonomic RightClaw's dashboard/gateway control plane already supersedes — nothing here argues for adding a runtime provider-write REST surface to RightClaw.


---
---

# Round 9 — CORRECTED FRAME: adopt opencode/MiMo vs build-on-rig (sandbox/egress excluded)

> **Supersedes round 7's "excluded as base" verdict, which was WRONG:** it penalized opencode/MiMo for their own sandbox/egress/default-provider posture, all of which are MOOT because RightClaw provides isolation via OpenShell. This round re-evaluates on the real axes (multi-model, grafting, adopt-vs-build), with sandbox/egress off the table. Verify 10/10. **Also overturns rounds 5–6 on subscription-OAuth — see §6 (now ToS-prohibited).**

# ROUND-9 FINAL DECISION: Adopt opencode/MiMo-as-harness vs Build-on-rig

*Multi-model RightClaw. Sandbox/egress OFF the table (OpenShell owns isolation). Migration cost free. For a Rust engineer who read rounds 1–8.*

## 1. Frame correction

Prior rounds penalized opencode/MiMo for harness-default posture — bundled sandboxing, default providers, default egress, host-fingerprinting plugins (mimo-free). **That was wrong for RightClaw and is excised from this round's scoring.** RightClaw supplies *all* sandboxing, network egress control, and credential isolation itself via OpenShell (k3s + TLS-MITM proxy + allowlists + host-side credential gateway). A candidate harness runs inside a throwaway OpenShell container; its own defaults are disabled or replaced and every byte of egress is governed by our policy. The only axes that matter:

1. Reliable structured output + tool-calling on non-Claude/self-host models.
2. How cleanly RightClaw's load-bearing pieces graft per-agent.
3. Complete-harness-for-free vs build-on-rig, with migration cost free.
4. One harness running Claude-subscription-OAuth *and* self-host per agent.

Sandbox/egress are off the table and do not count against either candidate. The TS/Bun-runtime objection that remains (§5) is a *runtime-image-weight and separate-process* operability concern, not an egress/posture concern — a legitimate axis, kept.

**Source-naming discipline** (the evidence set was sloppy here — fixed): graft/system-prompt/credential claims were read from **`XiaomiMiMo/MiMo-Code`** (the fork RightClaw would adopt); the open structured-output bugs (#15226, #8184, #17765) are filed against **`anomalyco/opencode`**; the thinned `dev`-branch prompt route is **`sst/opencode`**. "opencode" below means the family; each claim cites its specific fork.

## 2. Recommendation

**BUILD on rig — #1, MEDIUM confidence, by a THIN margin, conditional on one spike.**

The margin is **narrower than rounds 1–6 implied (~20% thinner once the rhetoric is corrected)**, and — critically — the **load-bearing reason has changed**. Earlier rounds led with structured output. After honest narrowing (§3), structured output is a *tie-breaker pending a spike*, not the headline. The durable, un-attacked grounds for rig #1 are now:

1. **Operability:** rig is an in-process Rust library — no second runtime, no per-agent server process, no host→sandbox transport, no session-persistence surface. opencode/MiMo is a per-agent Bun HTTP server with all four.
2. **Compile-time invariant enforcement:** today's per-turn contract is enforced at compile time by the `ClaudeInvocation` builder (Rust; invariant flags cannot be omitted — ARCHITECTURE.md). Adopting opencode moves it to an HTTP body against a TS server; the compiler stops being the enforcer.
3. **Own-the-loop fit** for RightClaw's deterministic per-turn learning pipeline + idle-compaction-as-judged-turn. Softer than 1–2: opencode's hook surface is a *superset* of the Agent SDK's, so this is a re-graft cost, not a blocker.

Structured output is demoted to **spike-conditional tie-breaker**: rig's native enforcement is real but applies only on tool-free / post-tool-result turns on vLLM self-host (§3). If spike #1 shows MiMo's *emulated* path achieves acceptable schema-conformance on the real target self-host models, opencode's largest technical disadvantage collapses and **adopt-MiMo becomes defensible** given free migration and a genuinely superset hook surface. The flip condition is empirical, not architectural.

**Not hybrid.** Two harnesses double the per-turn contract surface, credential plumbing, and invariant-enforcement story for no clear gain. Pick one loop.

## 3. The crux: reliable structured output + tool-calling on non-Claude models

**opencode/MiMo: emulated, with ONE confirmed self-host bug (not two).**

Both base opencode and MiMo register a synthetic `StructuredOutput` tool and set `toolChoice:"required"` only for `json_schema` format — **no `response_format`/`guided_json` anywhere in source** (grep returns zero hits in both `prompt.ts` files; verdict **confirmed**, claim 1). Base opencode retries 0 (`StructuredOutputError{retries:0}`, breaks immediately); MiMo hardened this with a bounded repair retry defaulting to 2 (`retryCount` default 2 at `message-v2.ts:79`; `autoRetryStructuredOutput` enforces `structuredRetries >= limit`) — a real robustness gain over base that a rig build would also have to implement.

But the retry is **downstream of the provider rejection**, so it cannot fix:

- **#15226 (OPEN, `anomalyco/opencode`) — the one confirmed self-host bug.** `toolChoice:"required"` is rejected by thinking-enabled reasoning models (Kimi K2.5): *"tool_choice required is incompatible with thinking enabled."* This is **opencode-specific** — it stems directly from the forced-tool emulation. MiMo's retry **cannot recover it**: re-issuing the same forced-tool request just re-hits the 400. This bites exactly RightClaw's target model class.
- **#8184 (OPEN, `anomalyco/opencode`) — NOT a rig advantage; UNKNOWN-symmetric.** Empty-parameter tool schemas fail strict validation on SGLang/vLLM (*"None is not of type array on schema required"*). **This is a tool-schema-emission bug, not a structured-output bug** — it would bite opencode's normal MCP tool calls too. Critically, **rig also emits tool schemas to vLLM and was never audited for the same class of bug.** Until checked (does rig emit `required: []`/`null` for parameterless tools?), #8184 is **evidence for neither side**. The round's earlier "two open bugs" framing is downgraded to **one confirmed (#15226) + one unaudited-symmetric (#8184)**.

Two further emulation notes:
- `toolChoice:"required"` forces *some* tool, not a specific one — but this is **not a rig advantage** (verdict **refuted**, claim 4): the AI SDK exposes a typed tool-specific choice `toolChoice: {type:'tool', toolName}` (`ToolChoice = 'auto'|'none'|'required'|{type:'tool';toolName}`), so a harness *can* pin the exact tool and avoid the wrong-tool burn. The draft's own self-refutation is correct.
- Forcing a tool call every turn while RightClaw's full MCP aggregator toolset is exposed may degrade normal tool selection on weak models — **UNKNOWN**, untested for RightClaw's tool surface.

**rig: native, server-enforced — but suppressed on initial tool turns, and absent on hosted DeepSeek.**

rig maps `output_schema` to `response_format {type:"json_schema", strict:true}` and merges it into the request (verdict **confirmed**, claims 2/5/6/7; `crates/rig-core/src/providers/openai/completion/mod.rs`). vLLM enforces this via xgrammar/guidance — but enforcement is **request-conditional**: vLLM's "supported by default" means *no extra server config*, enforced **only when the schema is sent** (`if response_format is not None:`; verdict **partial**, claim 6). vLLM deprecated `guided_json` in v0.12.0 and made `response_format json_schema` the recommended path — retiring a prior-round unknown. rig sends it; that's the differentiator. *Honest caveat:* "enforced when sent," not "imposed on every request."

The two catches that narrow the edge:
- **Suppressed on initial tool turns** (verdict **confirmed**, claims 2/7): rig gates `response_format` to `output_schema.is_some() && (tools.is_empty() || history_has_tool_result)`. On an *initial tool-present turn with no prior tool result* — which is **every RightClaw turn that calls the MCP aggregator**, the common case — rig **suppresses** the schema (a deliberate llama.cpp footgun dodge; rig's own comment, asserted by in-tree tests `request_conversion_omits_response_format_on_initial_tool_turn`). So on the common turn shape, rig does **not** enforce per-turn JSON either.
- **Hosted DeepSeek: no rig penalty, but no rig edge** (verdict **partial**, claim 3): hosted DeepSeek allows only `text`/`json_object`, never `json_schema`. But rig's DeepSeek provider sends **no `response_format` at all** (`warn!` + drop) and its Extractor falls back to tool-emulation for all providers — the *same* emulation opencode uses. rig's native advantage is **specifically a vLLM self-host advantage**, not a hosted-DeepSeek one.

**Bottom line on the crux.** rig's structured-output edge is real but **much narrower than the headline**: it applies to vLLM self-host, **only on tool-free or post-tool-result turns**, not to hosted DeepSeek, and not on the common MCP-tool turn. The fix for *either* harness is identical and available to both — split turns so a final tool-free turn carries the schema (rig's own `tools.is_empty()` branch re-applies it; opencode can issue a final schema-bearing turn too). On the turn shape where rig *does* enforce, it gets server-guaranteed conformance while opencode still emulates — that is the honest, residual edge. The decisive **UNKNOWN**: does MiMo's emulation achieve acceptable conformance on vLLM-served DeepSeek/Kimi/GLM/MiMo? **Untested — this is spike #1, and it decides the round.**

## 4. Grafting RightClaw onto opencode/MiMo per-agent

Mostly clean, with sharp edges. The hook surface is a **superset** of the Claude Agent SDK's. All claims below read from `XiaomiMiMo/MiMo-Code`.

- **System prompt — clean (verdict confirmed, sysprompt-replace).** `agent.prompt` *replaces* (ternary, not augments) the built-in provider prompt; system block = `[agent.prompt OR provider(model)] + caller system + per-message user.system` (`llm.ts:242-254`). RightClaw owns the identity block per agent.
- **Forced memory block — NOT the burden the finder claimed (verdict partial, sysprompt-memory-injection).** opencode unconditionally pushes a MEMORY.md teaching block for non-system actors with no config flag — **BUT** `run()` honors `input.prebuiltSystem ?? buildSystemArray(...)`, so passing a verbatim `prebuiltSystem` array **bypasses memory injection entirely**. No source fork, no transform-hook strip required. RightClaw's Hindsight-via-MCP memory ports as a remote MCP server; the redundant own-memory block is trivially avoidable.
- **Per-prompt control — clean (verdict confirmed, perprompt-fields).** The POST prompt route accepts full `PromptInput` per call: `model{providerID,modelID}`, `modelRef` (tier), `agent`, `format` (json_schema), `system`, `parts` (`session.ts:925-954`).
- **Per-agent MCP — clean via per-directory.** Config (incl. MCP servers) resolves per-directory via `InstanceMiddleware` (request carries `directory`/`x-mimocode-directory`); StreamableHTTP transport takes arbitrary `headers`, so RightClaw's per-agent Bearer goes in `mcp.headers`; runtime `add()` exists.
- **Per-agent credentials — THE HARD EDGE, with a load-bearing UNKNOWN.** Auth is a single **global** `auth.json` keyed by `providerID`, NOT per-directory (`auth/index.ts`). Two agents both using provider `"anthropic"` with different tokens **collide**. Resolution is **either** one server per agent **or** distinct providerIDs per agent (`anthropic-himA`) in per-directory config. **Whether a custom providerID resolves to the anthropic SDK adapter cleanly is UNKNOWN** — and the entire "one-server-per-agent erases the multi-tenant advantage" argument in §5 *depends on this resolving badly.* If custom providerIDs do map cleanly, single-server multi-tenancy survives and the operability gap shrinks. Spike #2 decides; §2/§5 must not assume the pessimistic branch.
- **Learning / reflection / compaction / NDJSON — richly graftable.** Hooks: `chat.message`, `chat.params`, `chat.headers`, `permission.ask`, `tool.execute.before/after`, `experimental.chat.system.transform`, `experimental.session.compacting`, `actor.preStop/postStop`. `actor.postStop` with `continue=true` injects a synthetic user message and runs another turn — a direct analog of `reflect_on_failure`. The `/event` SSE stream (heartbeated) = NDJSON logging.
- **Unchanged liabilities:** SKILL.md `allowed-tools` silently ignored (`skill/index.ts` parses only `{name,description,hidden}`) — RightClaw's per-skill tool restriction not honored (low stakes); prompt caching gated to Anthropic-family (inert on self-host, **same as rig**).

**One-server-per-agent?** *Conditional on spike #2.* If the providerID-disambiguation UNKNOWN resolves badly, the safe topology is one opencode-serve process per agent sandbox — same per-agent-process resource story as today's `claude -p`, but it erases the multi-tenant-single-server advantage. Whether per-directory isolation survives concurrent multi-tenancy in one process (separate MCP pools, no `Duration.infinity` global-config-cache or global-auth.json leakage) is **UNKNOWN** — needs a live two-agent concurrency test.

## 5. Adopt-vs-build tradeoff

**Free with opencode/MiMo:** a complete model-agnostic loop (models.dev + any OpenAI-compatible endpoint), full MCP client (stdio/StreamableHTTP+Bearer/SSE/OAuth), session fork/resume, per-request model selection, per-model reasoning-quirk handling (verdict **confirmed**, claim 5: deepseek empty-`reasoning_content` round-trip, `enable_thinking` for kimi/qwen/glm — free vs re-deriving on rig; `transform.ts`), and a hook surface that is a superset of the Agent SDK's.

**The hook surface IS enough control without owning the loop** for the mechanical pieces (system, MCP, reflection-via-postStop, logging-via-event, compaction-via-hook). The honest cost is **determinism over an ungoverned loop**: RightClaw's per-turn learning pipeline, idle-compaction-as-judged-turn, and closed-box invariants are RightClaw-owned mechanisms re-grafted onto someone else's agentic loop. With rig you own the loop and graft nothing — every invariant is first-class in-language.

**Adopt-cost ledger (operability + maintenance):**
- A second TS/Bun runtime in the closed-box image — but **single self-contained binary via `bun build --compile`**, tens of MB, comparable to today's `claude` binary (verdict **confirmed**, op-single-binary). The heavy-`node_modules` objection is neutralized; the separate-process objection is not.
- A new long-running per-agent server process + a host→in-sandbox-server transport with a **liveness signal AGENTS.md requires be direct, not connect-success** (opencode serve binds `127.0.0.1` by default — needs a sandbox-reachable bind under OpenShell policy; **UNKNOWN** transport/health design).
- A documented **session-persistence-loss bug class** across restarts (#17765, mostly Desktop/Windows; headless server-restart resume path **UNKNOWN**). RightClaw's resume/fork contract depends on persistence surviving server restarts.
- **Compile-time invariant loss** (verdict confirmed, graft-invariant-enforcement-loss): the per-turn contract moves from a compile-time-enforced Rust builder to an HTTP body against a TS server. RightClaw re-establishes equivalents in the Rust HTTP-driver layer but loses the compiler as enforcer.
- **Fast-moving-fork maintenance tax (added per critique).** MiMo-Code is a fork that hardened one path while diverging from `sst/opencode` — whose `dev` branch did a v2 rewrite that *thinned* the prompt route (verdict confirmed, opencode-dev-thinner-route: `dev` payload is only `{id, prompt, delivery, resume}`; MiMo kept the rich one). Adopting MiMo means tracking a fork of a fast-moving upstream and re-rebasing RightClaw's plugin graft against both. A recurring tax rig (own-the-loop, in-language) does not pay.

rig pays none of these: in-process Rust library, no server, no transport, no persistence surface, retained compiler enforcement, no upstream fork to track.

## 6. Subscription-OAuth + per-agent multi-provider in one harness

**This is NEUTRAL — it must not tip the harness decision in either direction, and the prohibited-path artifacts cancel.** The decisive new fact is the canonical Anthropic legal page (live, June 2026; quote verified verbatim): OAuth is *"intended exclusively"* for native Anthropic applications, and *"Anthropic does not permit third-party developers to ... route requests through Free, Pro, or Max plan credentials"*; developers *"should use API key authentication."* Anthropic *"reserves the right to ... enforce ... without prior notice"* — and already has: the de-facto opencode anthropic-OAuth plugin was **removed from npm following a legal request**, and the surviving community fork carries a ToS-violation disclaimer. The June-15 Agent SDK credit pool covers third-party apps only *"through the Agent SDK,"* not raw-OAuth HTTP clients.

Consequence:
- **Both opencode and rig would replicate the same now-prohibited raw-OAuth wire** if they self-attach the token. Neither has a blessed path.
- The artifacts cancel: opencode shipping no in-core OAuth (the plugin Anthropic took down) is an **opencode-negative for a prohibited path**; rig being able to vendor an Anthropic-OAuth provider in-tree (the merged chatgpt OAuth template, 26.5KB, copyable) is a **rig-positive for the same prohibited path**. Both describe a capability both must abandon. **They cancel to zero — neither appears in the comparison.** (Per critique D4, the residual "opencode worse-positioned" line is cut.)
- Per-agent multi-provider *itself* is clean on opencode (per-instance auth + per-request providerID/modelID — verdict confirmed, c8) and is the loop you build on rig.

**The ToS-clean path is identical for both and is what RightClaw does today:** spawn the real `claude -p` binary in-sandbox for the subscription/credit-pool path, API keys for everything else. **Subscription-OAuth is off the harness axis entirely** — neutralized, dropped from the ranking criteria. The remaining UNKNOWN (does any raw-OAuth path draw from the credit pool; will RightClaw ship one at all) is a **product/legal decision, harness-independent**, not a feature gap.

## 7. Honest re-rank + ranked spike list

**RE-RANK** (sandbox/egress excluded, multi-model the goal, migration free):

1. **BUILD-on-rig — #1, MEDIUM confidence, THIN margin.** Load-bearing grounds, in priority order: **(a) operability** (in-process Rust library; no second runtime/server/transport/persistence surface); **(b) retained compile-time invariant enforcement** (`ClaudeInvocation` builder vs HTTP body against a TS server); **(c) own-the-loop fit** for the deterministic learning pipeline + idle-compaction-as-judged-turn. **Structured output is a spike-conditional tie-breaker, not a primary ground** — native enforcement on tool-free/post-tool-result turns only, one confirmed opencode-specific bug (#15226), one unaudited-symmetric bug (#8184). Does **not** flip on the corrected axes — but the verdict is robust *because* it stands on (a)+(b) even if the structured-output edge collapses to near-parity.
2. **ADOPT opencode/MiMo (MiMo as graft target) — #2, genuinely viable, fastest-to-multi-model.** MiMo > base opencode (bounded structured-output retry; per-prompt `format`/`system` route; `sst/opencode`'s `dev` v2 rewrite thinned the prompt route — MiMo kept the rich one). Held back by: emulated structured output with one open self-host bug, ungoverned loop, second TS/Bun runtime + new server/transport/persistence surface, global-auth.json credential collision (possibly forcing one-server-per-agent — UNKNOWN), legally-dead anthropic-OAuth plugin, and a fast-moving-fork maintenance tax.

**Subscription-OAuth dropped from the ranking criteria entirely** — neutral for both.

**Ranked spike list — do #1 before committing:**

1. **DECISIVE — structured-output spike.** Stand up vLLM + ≥2 target self-host models (DeepSeek + Kimi or GLM). A/B: rig native `response_format json_schema strict` conformance + tool-calling success vs MiMo emulated path (synthetic tool + `toolChoice:required` + `retryCount=2`) on RightClaw's real Claude-tuned schemas. Record schema-conformance rate, retry-exhaustion rate, and whether #15226 (thinking model) actually fires. **This single result decides the round** — if MiMo emulation is good-enough, adopt becomes defensible.
2. **Credential-collision + concurrency spike.** Verify whether per-directory config with distinct providerIDs (`anthropic-himA`) resolves to the anthropic SDK adapter, OR confirm one-server-per-agent is forced. Run a two-agent concurrency test in ONE server for `Duration.infinity` config-cache / global auth.json / MCP-pool leakage. **§2 and §5 ranking prose must not assume the pessimistic branch until this resolves.**
3. **rig tool-schema audit (cheap, do alongside #1).** Does rig emit `required: []`/`null` for parameterless tools — i.e. does it hit #8184's class on vLLM/SGLang strict mode? Until checked, #8184 is evidence for neither harness.
4. **Transport + persistence spike (only if adopt survives #1/#2).** Bind opencode-serve to a sandbox-reachable interface under OpenShell policy with a **direct** liveness signal (not connect-success — AGENTS.md forbids). Verify HTTP-driven session resume/fork survives a serve-process restart headless.
5. **Product/legal decision (parallel, harness-independent).** Decide whether RightClaw ships ANY raw subscription-OAuth path or restricts the Claude-subscription route to spawning real `claude -p` in-sandbox. Neutralizes subscription-OAuth as a harness axis regardless of #1–#4.
6. **If build-rig confirmed:** port MiMo's free wins into the rig loop — bounded structured-output repair retry, per-model reasoning-quirk handling (deepseek `reasoning_content` round-trip, `enable_thinking`), and the turn-splitting discipline that lands the schema on a tool-free turn.

**Verdict: BUILD on rig (#1), thin margin, conditional on spike #1 — and for the *right* reasons: operability and retained compile-time invariant enforcement, not the structured-output edge, which §3 narrows to a tool-free-turn tie-breaker. Subscription-OAuth is neutralized for both and excluded. Overturn to ADOPT-MiMo only if emulated structured output proves good-enough on the target self-host models and time-to-multi-model dominates.** Sandbox/egress excluded throughout.

*Evidence hygiene caveat carried for implementers: the evidence URLs are pinned to moving `main`/`dev` branches (rig line numbers already drifted ~1058 vs the cited 1267 between research and write-up). Re-pin every load-bearing URL to a commit SHA before acting on a quote.*
