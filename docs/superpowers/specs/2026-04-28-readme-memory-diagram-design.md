# README memory diagram + memory copy fix

**Date:** 2026-04-28
**Status:** Spec — pending user review

## Problem

The Mermaid architecture diagram in `README.md` (lines 108-136) draws each agent's memory as `M1[(Memory · identity)]` *inside* its sandbox subgraph, with edges fully internal to the sandbox. This is incorrect for the headlined memory backend:

- **Hindsight mode** — the actual memory store is Hindsight Cloud (`api.hindsight.vectorize.io`), reached via MCP tools (`memory_retain` / `memory_recall` / `memory_reflect`) that the host-side aggregator exposes. Nothing about Hindsight memory lives inside the sandbox.
- **`MEMORY.md` mode** — the file is on the host at `agents/<name>/MEMORY.md`, mounted into the sandbox. The bot reads it for system-prompt injection. Calling it "in the sandbox" is half-true at best.

The current node also conflates two different things — agent identity (host-side `IDENTITY.md` / `SOUL.md` / `USER.md`, one set per agent, semantically tied to that sandbox) and memory (host-side or cloud, never sandbox-internal).

Two related copy mismatches:

1. **README "Memory" section** (lines 86-92) treats Hindsight and `MEMORY.md` as equal options. They are not — Hindsight is the recommended primary mode; `MEMORY.md` is a fallback for users who do not want a cloud dependency.
2. **`ARCHITECTURE.md` "Memory" subsection** labels File mode as "(default)" and Hindsight mode as "(optional)". This is technically true of the deserialization default in `crates/right-agent/src/agent/types.rs:130` but contradicts the recommendation. The "default" label here belabors a code-level detail that does not matter at the architecture-doc level.

## Goals

1. Diagram tells the truth about where memory lives (host-side or cloud, never sandbox-internal).
2. Diagram separates identity (per-agent host file, mounted into sandbox) from memory (cloud service via aggregator).
3. README "Memory" section frames Hindsight as primary, `MEMORY.md` as fallback.
4. README intro line 61 (under "Memory and evolving identity") leads with Hindsight, mentions file as fallback.
5. `ARCHITECTURE.md` "Memory" subsection drops `(default) / (optional)` framing in favor of `(primary) / (fallback)`, swaps order to lead with Hindsight.

## Non-goals

- Changing the code default of `MemoryProvider::File`. The wizard at `crates/right/src/wizard.rs:886` already promotes Hindsight when the user provides an API key during init.
- Depicting `MEMORY.md` mode in the diagram. It is a fallback and adding it bloats the diagram for a path most users will not take.
- Promoting Hindsight to a peer of MCP externals (Linear / Notion / Gmail). Memory is a headlined feature in the README pitch and earns its own visually distinct node.

## Design

### 1. Diagram replacement (`README.md` lines 108-136)

Replace the existing Mermaid block with:

````mermaid
flowchart LR
  U[You] --> TG[(Telegram)]

  TG <--> B1[Bot · agent one · host]
  TG <--> B2[Bot · agent two · host]
  B1 --> A1
  B2 --> A2

  subgraph SANDBOX_1["Sandbox · agent one"]
    A1[Claude Code]
    I1[(Identity)]
    A1 <--> I1
  end

  subgraph SANDBOX_2["Sandbox · agent two"]
    A2[Claude Code]
    I2[(Identity)]
    A2 <--> I2
  end

  A1 -.MCP calls.-> AGG
  A2 -.MCP calls.-> AGG
  AGG[MCP Aggregator · host] -->|holds tokens| EXT[(Linear · Notion · Gmail · …)]
  AGG -->|memory tools| HS[(Hindsight Cloud)]

  style SANDBOX_1 fill:#161616,stroke:#E8632A,color:#ddd
  style SANDBOX_2 fill:#161616,stroke:#E8632A,color:#ddd
  style AGG fill:#0f0f0f,stroke:#6bbf59,color:#ddd
  style HS fill:#0f0f0f,stroke:#6b8fbf,color:#ddd
````

Specific edits:

- `M1` / `M2` renamed to `I1` / `I2` and labels changed from `(Memory · identity)` to `(Identity)`. Identity is one set of agent-owned files per agent, tied to that sandbox semantically.
- New node `HS[(Hindsight Cloud)]` added outside both sandbox subgraphs.
- New edge `AGG -->|memory tools| HS` showing that Hindsight is reached via the aggregator (matches the actual code path where `memory_*` tools live in the aggregator's RightBackend).
- New `style HS` line picks a blue (`#6b8fbf`) stroke distinct from the orange sandbox stroke and the green aggregator stroke, marking memory as its own concern.

### 2. README "Memory" section (lines 86-92) — full replacement

````md
### Memory

The primary path is **Hindsight Cloud** — a managed semantic memory service. Append-only: every turn auto-retains a delta, next turn auto-recalls what matters. Per-chat tagging, prefetch cache. The agent remembers who it is talking to, what it was working on yesterday, and which stack the user runs — without replaying the whole transcript.

A fallback is available — **`MEMORY.md`** — a local file the agent curates itself via Claude Code's Edit/Write tools. No semantic recall, no per-chat tagging; just a markdown file the agent maintains. For anyone who does not want a cloud dependency.

Either way, memory survives restarts. Nothing resets when you `right up` again.
````

### 3. README "Memory and evolving identity" intro (line 61)

Current:

> Managed with Hindsight Cloud for semantic recall (append-only), or as a plain `MEMORY.md` file the agent curates itself. Either way, memory survives restarts and compounds over time. Each agent also writes its own identity and personality on first launch. Details below.

Replacement:

> Hindsight Cloud is the primary backend — semantic recall, append-only, per-chat scoped. A local `MEMORY.md` fallback is available for users who do not want a cloud dependency. Either way, memory survives restarts and compounds over time. Each agent also writes its own identity and personality on first launch. Details below.

### 4. `ARCHITECTURE.md` "Memory" subsection — reframe

Current:

```
**File mode (default):** Agent manages `MEMORY.md` via CC Edit/Write.
Bot injects file contents into system prompt (truncated to 200 lines).
No MCP memory tools.

**Hindsight mode (optional):** Hindsight Cloud API (`api.hindsight.vectorize.io`),
one bank per agent. Three MCP tools exposed via aggregator:
…
```

Replacement (swap order, swap labels, keep all technical detail):

```
**Hindsight mode (primary):** Hindsight Cloud API (`api.hindsight.vectorize.io`),
one bank per agent. Three MCP tools exposed via aggregator:
… (rest of paragraph unchanged)

**File mode (fallback):** Agent manages `MEMORY.md` via CC Edit/Write.
Bot injects file contents into system prompt (truncated to 200 lines).
No MCP memory tools.
```

The opening sentence "Two modes, configured per-agent via `memory.provider` in agent.yaml:" stays as-is.

## Out-of-scope concerns deliberately left untouched

- The code's `#[default]` annotation on `MemoryProvider::File` is preserved. Reframing the architecture doc as "primary / fallback" is a documentation change, not a code-default change. If the team later wants the code default to follow the recommendation, that is a separate spec.
- `crates/right-agent/src/agent/types.rs:129` carries the doc comment `/// File-based memory (MEMORY.md) — default.` — left alone for now. Could be updated in a follow-up to read `/// File-based memory (MEMORY.md) — fallback when no Hindsight API key is configured.` but is not user-facing and does not block this change.

## Verification

1. Render the README locally (any Mermaid renderer — GitHub preview is sufficient) and confirm:
   - No memory node inside either sandbox subgraph.
   - `HS[(Hindsight Cloud)]` appears outside both subgraphs.
   - Edge `AGG -->|memory tools| HS` is present.
   - `EXT` still shows Linear / Notion / Gmail without Hindsight folded in.
2. Read the README "Memory" section top-to-bottom and confirm it leads with Hindsight, frames `MEMORY.md` as fallback.
3. Read the README intro line 61 and confirm same framing.
4. Read the `ARCHITECTURE.md` "Memory" subsection and confirm Hindsight comes first with `(primary)` label, File second with `(fallback)` label.

No code changes; no test runs needed. All changes are documentation.

## Files touched

- `README.md` — diagram block (lines 108-136), Memory section (lines 86-92), Memory-and-evolving-identity intro (line 61).
- `ARCHITECTURE.md` — Memory subsection under Data Flow.
