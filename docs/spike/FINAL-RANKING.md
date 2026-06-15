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
