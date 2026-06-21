# Harness Migration — Consolidated Final Ranking

Capstone tying together the whole arc (rounds 1–12 + the hands-on spikes). Per-option detail in the sibling docs: `harness-migration-research.md` (rounds 1–9: claude-p alternatives, Agent SDK, ACP, rig, opencode/MiMo, subscription-OAuth), `HERMES-CONTROL.md`, `SPIKE-RESULTS.md` (live structured-output conformance), `PI-REEVAL.md`, `GOOSE-EVAL.md`.

## The single biggest finding — the field converged on tool-call-as-control

**Three independent, serious agent platforms — Hermes (NousResearch), Goose (Linux Foundation/ex-Block), Pi (earendil-works) — ALL use mechanism C: native tool-calls + plain-text reply, and NONE force a json-schema envelope every turn.** opencode/MiMo *emulate* a forced envelope (synthetic `StructuredOutput` tool + `toolChoice:required`) and the live spike proved that path is **fragile on weak/open models** (ignored-forced-tool, nested-object stringification; only ~9/13 open models pass, Kimi/small-Qwen hard-fail on a Venice grammar bug). Claude-native and **RightClaw-today (`--json-schema` every turn)** are the outliers.

**Implication:** the structured-signaling DIRECTION is the real decision, and it points at **dropping the forced envelope** for tool-call-as-control (reply = text; cron = `[SILENT]` sentinel; attachments/classify/run_note = tool-call args) + a **coerce/repair-never-reject** layer for weak-model robustness. Harness-independent; decide it first.

## The two axes that separate the harnesses (once signaling = mechanism C)

1. **EMBED (own the loop, in-process) vs ADOPT (delegate to a subprocess/harness).** RightClaw has *already built* worker/cron/reflection/learning around a thin `ClaudeInvocation` contract → EMBED fits; ADOPT means throwing that away.
2. **Rust (no sidecar) vs TS/Bun (sidecar).** OpenShell already owns isolation, so runtime-language is *discounted* (a Rust-binary sidecar is only marginally better than Node) — but a TS toolchain on RightClaw's Rust workspace is real friction.

| Option | Embed/Adopt | Lang | Signaling | Forks | MCP client | Subagents | SKILL.md | Coerce/repair | Governance |
|---|---|---|---|---|---|---|---|---|---|
| **rig** | **EMBED** | **Rust** | you implement (B or C) | build (clone vec) | rmcp 1.7 | build | build | build (borrow) | 0xPlaygrounds, pre-1.0 |
| **Pi-vanilla** | **EMBED** | TS | **C native** | native session-tree (best) | build adapter | build | yes | yes, throws | multi-maint, npm, pre-1.0 |
| **Goose** | ADOPT (subprocess) | **Rust** | **C** | copy_session | **superset** (StreamableHttp+Bearer+OAuth) | first-class | yes (.claude) | toolshim.rs | **Linux Foundation (best)**; v2-rc churn |
| **oh-my-pi** | ADOPT | TS/Bun | **C** | yes | yes | yes | yes (.claude) | task-scoped | single-maint, Bun |
| **opencode/MiMo** | ADOPT | TS | emulated-A + C | forkSession | yes | task | allowed-tools ignored | minimal | SST / Xiaomi fork |
| **Hermes** | reference | Python | **C + repair-never-reject** | — | yes | yes | — | the cleanest | NousResearch |
| **claude-agent-sdk** | adopt | TS | A native | yes | yes | yes | yes | n/a | Anthropic — Claude-only → OUT |

## Consolidated ranking (RightClaw: Rust shop, OpenShell isolation, needs skills+subagents+forks+MCP+multi-model, already owns its loop)

> **⚠️ SUPERSEDED — read the CONSTRAINT UPDATE at the end of this file as authoritative.** This ranking assumes RightClaw is *willing to build* MCP and the skills plumbing, so it places the build-your-own (EMBED) options high (rig #1, vanilla-Pi #3). The user's hard constraint — **"won't build MCP or skills myself"** — overrides it: it **drops rig and vanilla-Pi** (rig has no skill system; vanilla-Pi has no MCP) and selects ADOPT-a-complete-harness. **Authoritative rank under the constraint: 1) Goose, 2) opencode/MiMo, 3) oh-my-pi; rig & vanilla-Pi excluded.** Note: it is *vanilla* Pi that drops; *oh-my-pi* (the fork, which DOES ship MCP) survives at #3.


**#1 — rig.** Only true Rust EMBED; owns the loop, fits RightClaw's existing `ClaudeInvocation` architecture. You implement signaling (mechanism C cleanly) and build forks/skills/subagents/learning — but RightClaw has **already built** most of that around `claude -p`, so the cost is largely sunk. Thin, **narrowing** margin. Borrow a coerce/repair layer + fork primitive. Caveat: rig fork = clone-the-vec (Pi's session-tree is nicer); pre-1.0 churn; most code to write.

**#2 — Goose** *(strong; the pick if delegating the loop is acceptable).* Best **complete** option; **dethrones the TS harnesses** on language (Rust subprocess) + governance (Linux Foundation, beats every single-maintainer). Skills/subagents/forks/MCP-superset/multi-model/caching out of the box. But **ADOPT-as-subprocess, not embeddable** (`Config::global()`×171, unpublished crate, edition 2021, ACP/goosed drive) → cede the loop, contradicting RightClaw's design.

**#3 — Pi-vanilla (embedded `pi-agent-core`)** *(dark-horse middle).* EMBED like rig but TS; mechanism-C native; **best fork primitive of all** (non-mutating session tree); SKILL.md loader; README defers sandboxing to OpenShell (tightest architectural fit). But: TS toolchain in a Rust workspace; vanilla has no MCP/subagents/learning (build); pre-1.0.

**#4 — oh-my-pi / opencode-MiMo** *(adopt-TS harnesses, batteries-included).* Everything installable, but TS/Bun sidecar + weaker governance, and MiMo's emulated-forced-tool structured output is the fragile path the spike exposed.

**Tier 2 — Hermes** = the **design reference** (Python), not a harness to adopt. Its tool-call-as-control + `[SILENT]` sentinel + **repair-never-reject** is the philosophy all three platforms validate and RightClaw should borrow.

**Out — claude-agent-sdk**: Anthropic-locked, fails multi-model/self-host.

## The decision, reduced

1. **Decide signaling first (harness-independent): move from the forced `--json-schema` envelope to tool-call-as-control** (text reply + `[SILENT]` + tool-arg side-channels) **+ build a coerce/repair layer.** The field (Hermes/Goose/Pi) converged here; the spike proved the forced envelope is fragile on the open models you want.
2. **Harness: rig stays #1** *if* "own the loop, Rust-native, no sidecar" is the priority (fits the existing architecture). **Goose is the strong alternative** *if* adopting a complete Rust framework (delegating the loop) is acceptable — best complete option, only Rust one. Pi-vanilla is the EMBED-but-TS middle.
3. **Borrow regardless:** the coerce/repair layer (Pi `validation.ts`/`json-parse.ts`, Goose `toolshim.rs`, Hermes `coerce_tool_args`), the `[SILENT]` sentinel, the bounded-continuation final-output pattern (Goose `final_output_tool.rs`).
4. **The one remaining spike:** structured-signaling reliability on weak self-host models via the *chosen* mechanism (tool-call args + repair) — confirm before committing the harness. (Forced-envelope side already measured; SPIKE-RESULTS.md.)

## Carry-forward facts (don't re-litigate)
- **Subscription-OAuth self-attach is ToS-prohibited** (Anthropic enforces; opencode's plugin was pulled). Only ToS-clean Claude-subscription path = spawn the real `claude -p`. Subscription is off the harness axis.
- **Sandbox/egress posture of any harness is moot** — OpenShell owns isolation.
- **`propertyNames` Venice-Kimi failure** is a provider-side grammar bug (injected by the stack, not RightClaw's schema; not model incapacity).
- **structured-output reliability is a model property**, not fixable by harness choice — which is why tool-call-as-control + repair (tolerate & fix) beats forced enforcement on weak models.

---

## CONSTRAINT UPDATE (2026-06-14) — "won't build MCP or skills myself" → ADOPT, Goose #1

User hard constraint: **not willing to build the MCP client or the skills (discovery/execution) plumbing.** This re-weights the ranking decisively away from build-your-own (EMBED) toward adopting a complete harness that ships both out of the box.

- **rig — OUT of #1.** Has MCP (rmcp) but **no skill system → you'd build skills.** Eliminated by the constraint.
- **Pi-vanilla — OUT.** Has SKILL.md but **no MCP by design → you'd build the MCP adapter.** Eliminated.
- Survivors (ship MCP **and** skills): **Goose, opencode/MiMo, oh-my-pi.**
- **Goose's only real demerit (`Config::global()`×171 / not-embeddable) EVAPORATES under ADOPT:** one `goose` subprocess per agent ⇒ per-process global config ⇒ no cross-agent leak. The embeddability objection only mattered for in-process embedding, which the constraint rules out anyway.

**Re-rank under the constraint:**
1. **Goose** — ships MCP (superset StreamableHttp+Bearer) + native SKILL.md (`.claude`) + subagents + forks; **Rust subprocess (no Node)**; **Linux-Foundation governance** (best bus-factor); mechanism-C (aligns with dropping the forced envelope). Demerit neutralized by adopt-mode. Remaining cost: v2-rc churn (pin + track ACP), and re-grafting RightClaw's orchestration onto Goose's machinery.
2. **opencode/MiMo** — ships both, but TS sidecar + emulated-fragile structured output + single-org governance.
3. **oh-my-pi** — ships both, but TS/Bun runtime + single-maintainer + `mnemopi` memory collides with Hindsight.

**Decision recorded: ADOPT, not EMBED. Goose is the recommendation.** RightClaw delegates the loop to a Goose subprocess (MCP + skills + subagents + forks for free) instead of building the harness on rig.

**Skills nuance (so the constraint isn't mis-scoped):** skill *discovery/execution* (the plumbing) = Goose provides natively (reads `.claude/skills`, picks up `rightx-*`). The skill *learning pipeline* (prefilter→probe-writer→curator that AUTHORS skills) = RightClaw's existing product feature, provided by no harness; it is separable (it writes SKILL.md files that Goose then discovers on reload) and re-grafts rather than being "built from scratch."

### Recorded next step — the ONE pre-commit spike

Drive a `goose acp` (or goosed-HTTP) subprocess as RightClaw's harness and measure:
1. **MCP**: attach RightClaw's `:8100/mcp` Bearer aggregator as a Goose `StreamableHttp` extension; confirm `mcp__right__*` tools are callable end-to-end.
2. **Skills**: drop a real `rightx-*` SKILL.md into the agent's `.claude/skills` and confirm Goose discovers + invokes it.
3. **Signaling (mechanism C)**: confirm reply=plain text, cron notify-vs-silent maps onto `[SILENT]` sentinel / a tool, prefilter classify maps onto a tool-call + per-recipe `final_output`; measure conformance on the weak self-host models (Qwen/GLM/Kimi via Goose's OpenAI-compatible provider) — does Goose's `toolshim.rs` coerce/repair salvage the nested-arg failures the MiMo spike showed.
4. **Re-graft cost**: can RightClaw keep its worker/cron/reflection/learning orchestration while driving Goose per-turn, or must it adopt Goose recipes/sessions wholesale? ACP wire-contract stability across v1→v2-rc.
5. **Per-agent isolation**: confirm one-goose-process-per-agent keeps `Config::global` + sessions + OAuth creds isolated (no cross-agent leak); set subagent `isolation.mode=none` (OpenShell is the isolation layer).

If the spike is clean → commit to Goose. If ACP churn / re-graft cost / signaling fragility bite → fall back to opencode/MiMo (same adopt model, TS) or reconsider rig only if willing to build skills after all.

---

## RANKING UPDATE (2026-06-19) — minimalism + byte-prompt-control becomes a primary axis; rig-skills cost verified-small → rig back to #1

Two new inputs overturn the CONSTRAINT-UPDATE rank (Goose #1):

**1. mimo's bloat is an ACTIVE conflict, not passive size** (verified, not size-aesthetics). MiMo ships its **own FTS-indexed Markdown memory** (`memory/{global,projects,sessions}/*.md` + `dream`/distill) parallel to RightClaw's Hindsight, and — decisive — its system prompt is **ADDITIVE and not byte-suppressible**: `LLM.buildSystemArray` always layers your `system` on top of the agent/provider prompt **and** an always-on memory-instructions section (skipped only for system-spawned actors); the spike found **no run/SDK field that fully suppresses it** (SPIKE-PREP.md C12). RightClaw needs byte-level composite-prompt control (it has it today via `claude -p --system-prompt-file`) — MiMo structurally cannot give it. `serve --pure` + a custom minimal agent trim the **passive** surface (TUI/web/plugins/49k-token build agent) but **cannot** disable the memory-instructions injection or the additive model. So mimo is **demoted hard** — exactly the thing the user is correctly souring on.

**2. rig's "no skills" is real but the build cost is verified-small** (GitHub issues, 2026-06-19). `repo:0xPlaygrounds/rig`: **#1264 "feat: Claude Skills" (OPEN)** — maintainer deprioritized it as the *Anthropic API* skills feature ("just use `additional_params()`", low priority); the asker's clarification that they mean **filesystem SKILL.md loading (Claude-Code/codex style)** is **unanswered**. **#1705 (MERGED) "Delete skills/rig directory"** removed a `skills/rig/SKILL.md`+references that was a skill *teaching a coding agent to write rig* (opposite direction). **Code search `SKILL.md` in rig = 0 hits.** So: **no native runtime SKILL.md loader, none planned.** BUT the endorsed DIY path is small — build it on rig's `tool` primitive (asker cites deepagents/pydanticAI tools-as-loaders): scan `.claude/skills/*/SKILL.md`, parse frontmatter, surface name+desc, load body on demand. The Anthropic-API skills passthrough is irrelevant to RightClaw (Anthropic-hosted, Claude-only — not ClawHub file-convention/multi-model). The hard part (the learning pipeline that *authors* `rightx-*` skills) is **already RightClaw's**; a harness only ever supplied discovery/execution.

This quantifies rig's previously-disqualifying "build MCP + skills" as: **MCP = wire rmcp 1.7 to `:8100/mcp` (a client exists, not protocol work); skills = a small `tool`-based filesystem loader.** Neither is the heavy lift the CONSTRAINT-UPDATE assumed.

**Re-rank (criterion now: minimalism + byte-control, which the user is weighting heavily):**
1. **rig** — only option delivering minimal surface + **byte-prompt-control** + **no parallel memory/session/prompt-injection** + Rust in-process + **native `response_format`** (good structured-output path, may dodge `propertyNames`). Build list bounded and mostly already-built (loop ported from existing `claude -p` orchestration; skill-loader small; rmcp wiring; borrow coerce/repair). **Cost: most own-code + maintenance, pre-1.0 churn, owns all glue.**
2. **Goose** — best batteries-included if unwilling to build even the loader / OK ceding the loop: Rust subprocess, MCP-superset + SKILL.md + subagents + forks, **opt-in memory** (cleaner than mimo), LF governance. **Cost: cede loop + prompt control (byte-controllability UNVERIFIED — pre-commit check), v2-rc churn, re-graft orchestration, carry 20-crate framework.**
3. **Pi-vanilla** — embed-but-TS: best fork primitive, has SKILL.md loader, OpenShell-aligned; cost = TS in a Rust shop + build MCP adapter (no rmcp equiv).
4. **opencode/MiMo** — demoted: active own-memory + non-suppressible additive prompt fight RightClaw's design; emulated-fragile structured output.
5. **oh-my-pi** — TS/Bun, single-maintainer, `mnemopi` collides with Hindsight.

**Swing factor #1↔#2:** willing to write a small `tool`-based SKILL.md loader + own more glue (→ rig, minimalism+control) vs want skills/MCP handed over + accept framework weight + ceding the loop (→ Goose). The minimalism/control priority the user now states → **rig #1**; this consciously softens the earlier "won't build MCP/skills" constraint, justified because that work is now shown to be wiring + a small loader, not protocol/pipeline construction. **Next verify (if Goose stays contender): Goose's system-prompt byte-controllability + whether anything is always-injected** (the mimo failure mode) — not measured in GOOSE-EVAL.

**rig #1 VERIFIED (2026-06-19, `docs/spike/RIG-DEEPDIVE.md`)** — due-diligence against current source (v0.39.0 @ 57de2005) + issues, supersedes stale rounds 1–9. **Capability = settled green:** byte-prompt-control CONFIRMED (preamble verbatim, no always-on injection — the thing mimo can't do); MCP rmcp StreamableHttp+Bearer → `:8100/mcp` CONFIRMED; prompt-caching first-class; 24 providers incl. `moonshot`(=non-Venice Kimi path) + OpenAI-compat self-host; static-musl FEASIBLE (rustls default, pin `ring`). **Structured-output+tools:** clean on Anthropic + OpenAI-Responses (native schema → no `propertyNames` artifact); the OpenAI-compat **self-host** path has first-turn suppression (#1622) + every-turn-schema bias (#1928, fix = unmerged PR #1929). **Risks sharpened:** churn HIGH (≥10 breaking in ~11 versions, no 1.0 roadmap — dominant cost), bus-factor MED-HIGH (recent core ~80% one dev), 2 silent stream-swallows to patch for fail-fast. Net: rig decision = absorb HIGH churn + own glue (subagents/fork/skill-loader + ~3 small patches) for full control + minimal surface + Rust-in-process → holds at #1 under the user's priorities.

---

## ARCHITECTURE CORRECTION + DEPLOYMENT-FIT (2026-06-19) — the agent runs IN the sandbox; EMBED-vs-ADOPT partly collapses; rig's lead NARROWS

**Correction to a load-bearing error in the framing above.** Earlier text called rig "EMBED / own-the-loop **in-process** / no-sidecar / minimal." That is WRONG for RightClaw's real architecture. Confirmed in `crates/bot/src/cc/invocation.rs:670-688`: a **sandboxed agent (the default)** is invoked as `ssh -F <cfg> openshell-<sandbox> '<claude -p --output-format stream-json --resume <id> …>'` — a **fresh process exec'd PER TURN over SSH, INSIDE the sandbox**; user msg via **stdin**, results stream back as **stream-json NDJSON over stdout**, OAuth token injected as sandbox env, `--mcp-config`/`--system-prompt-file` are sandbox-side paths, and the agent reaches `:8100/mcp` + the model API **from inside the sandbox** (OpenShell TLS-MITM egress). `guard_no_sandboxed_host_exec` (line 673) makes a sandboxed agent **refuse** to build a host command — fails closed. Only `sandbox: mode: none` agents exec on the host.

**Consequence:** rig cannot run in the host bot without breaking the security model (host egress / host tool-exec / host-side creds). rig = the engine of a **sandbox-side static-musl runner** (`right-runner`) you BUILD, upload to `/sandbox/.local/bin`, and exec over the SAME SSH slot `claude` occupies — driven per-turn, resume-by-session. The host bot drives it over SSH with a stream protocol **you design**. So **EMBED-vs-ADOPT partly collapses**: in RightClaw *everything* runs as a sandbox subprocess over SSH; rig is also that, just authored by you. rig's "minimal in-process" edge was partly illusory.

**Deployment-fit verified (per-turn-SSH-exec-in-sandbox model):**
- **Goose — CONFIRMED near-exact `claude -p` fit.** `goose run --output-format stream-json --resume --session-id <id> -i -`: one-shot per-turn, stdin prompt, NDJSON `StreamEvent` over stdout (incl. tool-call events), SQLite session resumed by id, all in ONE in-sandbox process, in-process rmcp `StreamableHttp` + **Bearer** (header carried in stored `config.yaml`, not the `goose run` flag — one seam, analogous to claude's `--mcp-config`). Ships **musl static** (`portable-default`, ~38 MB compressed; drops local-inference/V8 — fine for remote models), runs D-Bus-free (`GOOSE_DISABLE_KEYRING`), state pinnable (`GOOSE_PATH_ROOT`). **No host companion.** → Goose drops into the `claude` slot cleanly. (Refs: goose-cli `cli.rs` run/output-format/resume; `session_manager.rs` sqlite; `extension_manager.rs` rmcp StreamableHttp+headers; `.github/workflows/build-cli.yml` musl.)
- **mimo — PARTIAL.** Single bun-compiled binary (~100 MB, no JS runtime), stdin, NDJSON (opencode-shaped → parser rewrite), resume-by-id, self-contained in-sandbox MCP(HTTP/Bearer)+tools+SQLite, no host daemon — **BUT `mimo run --format` only accepts `default|json`; structured `json_schema` is NOT wired into the `run` CLI** (present in server/SDK, reachable only via `mimo serve` HTTP) → needs an upstream patch or driving the in-sandbox HTTP server. Plus the prompt-injection demerit (location-independent). Stays #4. (Refs: `cli/cmd/run.ts:244-249` format flag; `session/prompt.ts` json_schema in SDK only; `script/build.ts` bun --compile.)
- **rig — build the runner + protocol + upload/version yourself** (the EMBED cost, now explicit and larger than the in-process framing implied).

**Ranking impact:** NARROWS rig's lead. Goose is now a **verified clean drop-in** (prebuilt binary, ready stream-json protocol, in-sandbox MCP+Bearer, musl static) — exactly the `claude` slot. rig requires building+shipping+versioning the runner + designing its protocol. rig still leads on **byte-prompt-control + no-parallel-memory + native structured output** under the user's minimalism+control priority — but **rig-vs-Goose is now genuinely close**, hinging on ONE unresolved check: **is Goose's system prompt byte-controllable, or does it always-inject like mimo?** (the criterion that sank mimo — launched 2026-06-19, pending). If Goose gives full prompt control, its verified clean deployment fit + ships-everything makes it a very strong #1 contender; if it injects like mimo, rig stays #1.

**RESOLVED (2026-06-19) — Goose prompt-control = PARTIAL → rig #1 holds.** Goose is BETTER than mimo (it has a real full-replacement override `GOOSE_SYSTEM_PROMPT_FILE_PATH` → `Agent::override_system_prompt`; the "you are goose" identity / extension docs / tool-instruction blocks live in the built-in `system.md` template and ARE suppressible under override; tool JSON schemas go in a separate provider `tools` field like Claude). But it is NOT byte-exact, on two counts: **(1)** the override is rendered through **minijinja** (`prompt_manager.rs:162` → `prompt_template::render_string`), not emitted verbatim — `{{`/`{%`/`}}` in the composite prompt interpolate or error, plus `.trim()` + `sanitize_unicode_tags` mutation; **(2)** `system_prompt_extras` are unconditionally appended under `# Additional Instructions:` (`prompt_manager.rs:193-197`, no suppress flag) — and **Goose's structured-output mechanism IS a system-prompt injection**: `add_final_output_tool` (`agent.rs:938-957`, wired every turn via `apply_recipe_components`) appends a `# Final Output Instructions` block embedding the JSON schema (`final_output_tool.rs:81-92`). RightClaw uses structured output every turn → an unsuppressible second schema directive layered on its own composite prompt (per-turn token cost on cached turns + model-confusion risk). **So on BOTH axes rig is cleaner than Goose:** rig's own runner = full byte-exact prompt + native `response_format` (schema as a separate API field, NOT injected prompt text, no `propertyNames` on Anthropic/OpenAI). **Decision fully characterized — rig: build the runner, FULL control, HIGH churn; Goose: cleanest drop-in + ships-everything, but PARTIAL prompt-control + schema-injected-every-turn + framework weight + cede the loop. Under the user's byte-control+minimalism priority → rig #1, Goose #2.** (Goose's leak is bounded and would be fine for a platform NOT demanding byte-exact every-turn structured output — it's a RightClaw-specific demerit, not a universal flaw.)

**Note — mistral.rs is NOT on this axis.** `EricLBuehler/mistral.rs` (launched for research 2026-06-19) is an inference ENGINE (vLLM-class, Rust), i.e. the self-host model-SERVING layer a harness CALLS — not a harness competing with rig/Goose. Relevant to self-host + structured-output reliability (native constrained decoding → no Venice/`propertyNames` tooling bug), tracked separately.
