# Goose (aaif-goose/goose) — Harness Evaluation for RightClaw

**Bottom line up front:** Goose is genuinely Rust + complete + MCP-native, and it is the strongest *complete-framework* option in the field — but the central thesis ("Rust + complete + MCP-native dominates both rig and opencode") **fails on the load-bearing axis**. Goose is an app you **ADOPT as a subprocess** (ACP-over-stdio / goosed HTTP), not a stable crate you **EMBED** like rig. The Rust-language win does not translate into in-process control. Goose lands **roughly tied-with-or-slightly-behind rig**, clearly **above opencode/MiMo and Pi**.

All source claims below were re-verified against `aaif-goose/goose@main` (HEAD `9d166ece`, 2026-06-13). Where the draft's evidence package cited the wrong file or carried an internal contradiction, the text below is corrected to what the live source actually says.

---

## 1. Identity — aaif-goose vs block/goose

**RESOLVED: relocation, not fork. `aaif-goose/goose` is canonical and maintained.**

- `github.com/block/goose` returns `HTTP/2 301 → location: https://github.com/aaif-goose/goose`. The REST API redirects `repos/block/goose` to repository **ID 846698999**, which resolves to `aaif-goose/goose` with `fork:false, parent:null, source:null, created_at 2024-08-23`. Identical repo ID on both paths = same underlying repo object relocated via transfer/rename. `fork:false`/`parent:null` definitively refutes the fork hypothesis. (https://api.github.com/repos/aaif-goose/goose)
- The move is stated verbatim in the README: *"goose has moved from `block/goose` to the Agentic AI Foundation (AAIF) at the Linux Foundation."* Owner is the `aaif-goose` org (created 2026-03-25). (https://github.com/aaif-goose/goose)
- **Language:** Rust **64.2%** (corrected from GitHub byte counts: 6,735,482 / 10,490,201 = 64.21%; the draft's evidence first said 64.3% — wrong). 20-crate Rust workspace.
- **Maturity/backing:** ~49.2k stars (49,206), v1.37.0 published 2026-06-03, last push 2026-06-13, `archived:false`. Weekly-to-biweekly release cadence; **a v2.0.0-rc is already pre-released (2026-04-27)** — breaking-change risk for anyone pinning the API. (https://github.com/aaif-goose/goose/releases)
- **Governance is the standout asset:** Block → Agentic AI Foundation / Linux Foundation. This beats *every* single-maintainer competitor (Pi/oh-my-pi) on bus-factor.

---

## 2. Library-vs-app — the crux (EMBED vs ADOPT)

**VERDICT: Goose is ADOPT-as-subprocess, NOT a clean embeddable crate like rig.** This is the decisive finding and it must not be inflated by "it's Rust."

The nuance matters, so both sides:

**Why it *looks* embeddable:**
- The core `goose` lib crate publicly exports the loop: `pub use agent::{Agent, AgentConfig, AgentEvent, ...}` and `pub async fn reply(&self, user_message, session_config, cancel_token) -> Result<BoxStream<'_, Result<AgentEvent>>>`, plus `add_extension`, `extend_system_prompt`, `add_final_output_tool`. So it *is* technically callable from Rust. (crates/goose/src/agents/agent.rs, agents/mod.rs)

**Why you should NOT embed it:**
1. **Not on crates.io, no semver.** `cargo add goose` pulls an **unrelated load-testing framework** (v0.18.1, Locust-inspired, owner Jeremy Andrews, repo tag1consulting/goose). The AI lib crate is named `goose` *inside* the workspace but is unpublished. Embedding = a **git dependency on an unversioned internal app crate** (optionally narrowed to `package = "goose"`). (https://crates.io/api/v1/crates/goose)
2. **Edition 2021, not RightClaw's mandated 2024.** Cargo permits mixing, but it is not a same-edition drop-in.
3. **Process-global config singleton.** The agent loop calls `Config::global()` **171 times** across the crate (9 inside `agent.rs` alone) — keyring + env-var + `~/.config/goose`-backed. This directly conflicts with (a) RightClaw's per-agent isolation and (b) AGENTS.rust.md's "never read env in `Default` / config flows through params" rule. `Agent::with_config(AgentConfig)` exists but does **not** eliminate the global reads. (crates/goose/src/agents/agent.rs)
4. **Block's own sanctioned surface is a separate process.** `goose-sdk` is the documented integration path and it is an **ACP-over-stdio client** to `goose acp` — its in-process uniffi surface is an explicit `ping→pong` stub ("*replace bindings with the actual implementation*"); it has **no dependency on the `goose` core crate** and physically cannot run a turn in-process. (crates/goose-sdk/src/lib.rs, bindings.rs)
5. **Drive surfaces are all subprocess:** `goose run` (stdin via `--instructions -`, `--input-text`, `--recipe`, `--params`), `goose acp` (ACP over stdio), a `goose serve` ACP-over-HTTP/WebSocket subcommand, and `goosed`/goose-server (axum HTTP + utoipa OpenAPI; Block is *migrating goosed to ACP-over-HTTP*, block/goose#6642). (crates/goose-cli/src/cli.rs)
6. **Weight:** embedding-as-git-dep pulls the full multi-crate workspace (vendors V8 for the desktop UI JS runtime + ACP/telemetry/desktop machinery).

**Implication:** Adopting Goose means RightClaw supervises a foreign Goose process with its own session store, config, and ACP protocol — a **control-plane shift, not a library call**. This is the same sidecar class as opencode/MiMo/Pi — *except the sidecar is a Rust binary, not Node*. That removes the foreign-runtime sting but **not** the "you don't own the loop" cost. **rig remains the only true EMBED option.**

> UNKNOWN (open): Could RightClaw vendor+patch `goose` to thread `AgentConfig` through all 171 `Config::global()` sites and drop the singleton, making it embeddable? Not estimated from source — would require reading every call site. If feasible, the embed objection partly dissolves — but you'd be maintaining a fork of a fast-moving (v2-rc looming) unpublished crate.

---

## 3. Signaling mechanism — C (tool-call-as-control), with soft opt-in A

**Default = mechanism C. Structured output = the *softest form of A*. No mechanism B anywhere in the request builders. Coerce/repair exists but is narrow.** Goose **inherits** weak-model structured-output fragility for forced output, but its *default* posture sidesteps it.

> Citation correction (draft FIX 1/2/3): the draft's evidence cited `crates/goose-providers/src/formats/openai.rs` and `…/anthropic.rs`. Those are the **wrong tree**. There are two provider format dirs: `crates/goose-providers/src/formats/` (contains only `openai.rs`) and the **live** `crates/goose/src/providers/formats/` (anthropic.rs, openai_responses.rs, google.rs, databricks.rs, …). The agent loop wires the **`goose` crate's** `providers::formats`. The signaling claims below were re-verified against the live files.

- **Default turn is plain text + native tool-calls (C).** The `reply()` loop streams `AgentEvent` items and dispatches native tool calls via `dispatch_tool_call`; the Provider trait is streaming-first; no json-schema envelope per turn. (crates/goose/src/agents/agent.rs, providers/base.rs)
- **Structured output is opt-in PER RECIPE, mechanism A (emulation).** A synthetic tool `recipe__final_output` carries the recipe's `json_schema`; the model is *instructed* ("You MUST call the `final_output` tool NOW") and, if it skips, a continuation message re-prompts and loops. Construction panics if the recipe schema is missing/empty. (crates/goose/src/agents/final_output_tool.rs)
- **Validation is client-side/local** via the `jsonschema` crate after the model emits the tool call — **NOT server-side** (the draft's evidence originally said server-side; corrected). Same emulation *family* as opencode/MiMo, but if anything *weaker* (no constrained decoding anywhere).
- **No forcing — precisely.** **There is no `tool_choice: required` anywhere in the repo.** But "no tool_choice at all" is **false**: `chatgpt_codex.rs:~388` does `payload_obj.insert("tool_choice", json!("auto"))` + `parallel_tool_calls: true`, and `githubcopilot.rs` has a `promote_tool_choice` post-processor. The live `anthropic.rs` and `openai_responses.rs` builders set **no** `tool_choice`. The substantive claim holds (no *forced* structured output); the wording is corrected to "no `tool_choice: required`." (crates/goose/src/providers/chatgpt_codex.rs)
- **`response_format` appears nowhere in the live builders.** Verified ABSENT in `anthropic.rs`, `openai_responses.rs`, and `chatgpt_codex.rs`. A user could only smuggle a native `response_format` in via the generic, arbitrary-key `request_params` passthrough bag — untested, and **not** a first-class "`response_format` mechanism." So **mechanism B is absent**; state it cleanly rather than as a hedge. (crates/goose/src/providers/formats/openai_responses.rs)
- **Validation is reject-and-retry, NOT Hermes coerce/repair-never-reject.** On mismatch, `final_output` returns `INVALID_PARAMS` + "Please correct your output… and try again," bouncing back to the model. (final_output_tool.rs)
- **`toolshim` is a real but DIFFERENT robustness layer.** It is a tool-call *emulator/interpreter* for models lacking native tool-calling: routes text through a second interpreter model (Ollama structured-output API, default `mistral-nemo`) to extract tool-calls, with a tolerant JSON parser (`parse_json_value_tolerant` repairs malformed backslashes, `extract_first_json_object` pulls JSON from prose). **Opt-in** via `ModelConfig.toolshim:bool`; strong models bypass it. It is NOT a malformed-native-arg repair layer. (crates/goose/src/providers/toolshim.rs, model.rs)
- **The continuation loop is bounded** (corrected from "open question"). The `FINAL_OUTPUT_CONTINUATION_MESSAGE` re-prompt sits inside the same loop as the turn guard: `DEFAULT_MAX_TURNS = 1000` (override via `session_config.max_turns` or `GOOSE_MAX_TURNS`), `if turns_taken > max_turns { … break; }`. Each re-prompt consumes a turn against the cap — no unbounded turn-burn. (crates/goose/src/agents/agent.rs)

**Net on the weak-model question:** For *forced structured output*, Goose is the **softest A** — instruction + bounded re-prompt loop + local validation, fully model-dependent. It is plausibly **at least as robust as opencode/MiMo's ~9/13** for weak models *because of* toolshim's repair/interpreter path — **but these are different mechanisms** (MiMo emulates a forced tool; Goose's default is plain tool-calls + an opt-in interpreter), and Goose's own pass rate on the same model set is **unmeasured**. Reliability remains a model property. **This directly confirms the user's established leaning:** text-reply + tool-calls + repair is a viable production design; blanket every-turn JSON is unnecessary.

> UNKNOWN: Whether the `request_params` `response_format` passthrough yields server-side constrained decoding (true B) on vLLM/Venice was not exercised. How the bounded continuation loop would compose with RightClaw's existing 3-rejection-abort+reflection was observed but not traced end-to-end.

---

## 4. Hard-requirement map — PROVIDES vs RightClaw BUILDS

| Req | Goose status | Evidence |
|---|---|---|
| **SKILLS — SKILL.md** | **PROVIDES.** Native `SKILL.md` per agentskills.io spec (frontmatter `name`/`description`/`metadata`), recursive discovery, surfaced as the **`skills` MCP platform extension** (`SkillsClient`, `load_skill` tool, progressive disclosure), `default_enabled:true`. **Discovery dirs (corrected, verified verbatim):** project scope pushes `.agents/skills`, `.goose/skills`, **and `.claude/skills`**; home scope pushes `.agents/skills`, the config dir, **`.claude/skills`**, and `.config/agents/skills`; plus installed-plugin dirs. The draft's evidence package elsewhere claimed Goose reads `.agents/skills` *"NOT `.claude/skills`"* — **that is false and is struck**; Goose reads `.claude/skills` at both project and home scope. Docs: *"compatible with Claude Desktop and other agents that support Agent Skills."* | crates/goose/src/skills/mod.rs (`all_skill_dirs`), client.rs |
| **SKILLS — per-skill allowed-tools** | **RightClaw BUILDS.** Goose's `SkillFrontmatter` struct parses only `name`/`description`/`metadata`; **no `allowed-tools` enforcement**. (The agentskills.io spec lists an optional `allowed-tools` field, but Goose does not parse/enforce it.) | crates/goose/src/skills/mod.rs |
| **SKILLS — learning loop** | **RightClaw BUILDS.** **No prefilter/probe-writer/curator** (grep: zero matches across crates). Goose does discovery + use + user/agent-invoked CRUD (`sources.rs`); it can author a skill on explicit instruction but never mines turns autonomously. | crates/goose/src/skills/, sources.rs |
| **SUBAGENTS** | **PROVIDES (first-class).** `run_subagent_task` / the `summon` extension's `delegate` tool spawns a fresh isolated `Agent` (`Arc::new(Agent::with_config(...))`; own provider/model/extensions/conversation), `max_turns`, cancellation, **structured return** (schema-validated `final_output` if recipe sets `response`, else text). Async/background (max 5 concurrent via `GOOSE_MAX_BACKGROUND_TASKS`, collect via `load(task_id)`). Recursion-blocked. Also discovers `.claude/agents/*.md`. | crates/goose/src/agents/subagent_handler.rs, platform_extensions/summon.rs |
| **CONTEXT FORKS** | **PROVIDES (fork-without-mutating-parent).** `SessionManager::copy_session` reads source read-only, writes full clone into a NEW session id; parent untouched. Exposed via ACP `fork_session` (**dispatched** — `dispatch.rs` routes `ForkSessionRequest` to `agent.on_fork_session(...)`; the `#[allow(dead_code)]` on the wrapper is indirection, not non-wiring) + CLI `--fork`. `truncate_conversation` + `replace_conversation` compose into fork-at-boundary. Sessions are a SQLite message log. | crates/goose/src/session/session_manager.rs, acp/server/fork_session.rs, acp/server/dispatch.rs |
| **MCP CLIENT (StreamableHTTP + Bearer)** | **PROVIDES — superset.** `ExtensionConfig::StreamableHttp { uri, headers: HashMap, env_keys, socket, ... }` → `StreamableHttpClientTransport::with_client(...).custom_headers(...)`, + `AuthClient`/`AuthorizationManager` OAuth + 401-fallback + HTTP-over-UDS. `${ENV}` header substitution **is tested** (`test_streamable_http_header_env_substitution`: `Bearer ${AUTH_TOKEN}`; plus `test_custom_headers_forwarded_to_http_extension` and `test_custom_headers_forwarded_oauth_path`). **RightClaw's `:8100/mcp` Bearer aggregator attaches verbatim.** SSE explicitly unsupported. rmcp **1.4** (RightClaw on 1.7 — both 1.x). | crates/goose/src/agents/extension_manager.rs, extension.rs |
| **MULTI-MODEL / SELF-HOST** | **PROVIDES.** ~30 providers (Anthropic/OpenAI/Ollama/OpenRouter/LiteLLM/xAI/Google/Bedrock/Azure/Databricks/HF/…), generic `OpenAiCompatibleProvider` (base_url + `/models`) for vLLM/Venice, local `candle`+`llama-cpp` (`local_inference`), **three Claude paths** (anthropic API, `claude_code` subprocess provider, `claude_acp`). | crates/goose/src/providers/init.rs, openai_compatible.rs |
| **PROMPT CACHING** | **PROVIDES.** Anthropic `cache_control: ephemeral` on last + second-to-last user messages (rolling) + last tool-spec (caches the whole tool prefix) + system prompt; also OpenRouter/LiteLLM/Bedrock/Databricks. (Verified in the **live** `crates/goose/src/providers/formats/anthropic.rs`.) | crates/goose/src/providers/formats/anthropic.rs |
| Session resume/id, compaction, scheduler/cron, retry | **PROVIDES.** SQLite resume by id; `compact_messages` (auto at `GOOSE_AUTO_COMPACT_THRESHOLD` + manual); `scheduler.rs`; per-recipe `RetryConfig`; headless `goose run`. | crates/goose/src/session/session_manager.rs, context_mgmt/mod.rs, scheduler.rs, recipe/mod.rs |

> UNKNOWN (verify before adopting): top-level **budget/cost cap** and a **failure-reflection primitive** (only subagent `max_turns` + compaction confirmed; parent-turn budget/reflection not traced); whether RightClaw's **claude_code provider** path preserves `--json-schema`/`--mcp-config`/`--resume`/caps; **NDJSON-equivalent** machine output mode for `goose run`; whether SKILL.md matches **ClawHub byte-for-byte** (registry/install conventions, not just frontmatter core).

---

## 5. The "Rust + complete + MCP-native" thesis — honestly tested

**The thesis is HALF-TRUE and FAILS on the axis that split the field.**

- **Rust:** yes (64.2%, 20-crate workspace).
- **Complete:** yes — skills, subagents, forks, recipes, providers, caching, scheduler, compaction all in source.
- **MCP-native:** yes — rmcp 1.4, extensions *are* MCP servers, StreamableHTTP+Bearer is a superset of RightClaw's needs.
- **"Dominates both axes": NO.** The field split on **EMBED vs ADOPT**. Goose does **not** collapse that axis — it lands firmly on the **ADOPT** side (subprocess over ACP/goosed; unpublished, edition-2021, `Config::global()`×171 internal crate). So:
  - **vs rig:** Goose **loses** in-process embedding and the thin compile-time-invariant surface. rig is a *published, semver'd, purpose-built embeddable* library where you own the loop. Goose's contracts are runtime/protocol, not your type system.
  - **vs opencode/MiMo:** Goose offers **Rust-as-subprocess instead of TS-as-subprocess**. But RightClaw's sandbox (OpenShell) already neutralizes the runtime-language concern (out of scope). So the Rust win here is *real but discounted* — it removes the Node objection and the bus-factor objection, but not the "foreign process / cede control plane" cost.

**Where the thesis genuinely pays off:** for a Rust+MCP shop, Goose ships 4 of the hard requirements (skills, subagents, forks, MCP client) + multi-provider + caching out of the box as a single Rust binary, eliminating most of rig's build-it-yourself burden *and* removing the Node sidecar. That is real. It just comes at the price of not owning the loop — which **contradicts RightClaw's existing `ClaudeInvocation`-centric, own-the-loop architecture** (worker/cron/reflection/learning already built around a thin invocation contract).

---

## 6. FINAL RE-RANK

For RightClaw (Rust shop; skills + subagents + forks + MCP + multi-model; has *already built* its orchestration loop):

1. **rig** — *leader, thin margin.* The only true EMBED: published, semver'd, clean, native `response_format` + `cache_control` + rmcp, you own the loop and compile-time invariants. You build skills/subagents/forks/learning — but RightClaw **has already built** most of that around `claude -p`. Fits the existing architecture.
2. **Goose** — *strong second; tied-with-or-slightly-behind rig.* Best *complete-framework* option. Decisive demerits: **not embeddable as a stable crate** (subprocess/ADOPT only), **`Config::global()` singleton** vs per-agent isolation, **edition 2021 + unpublished + v2-rc churn**, **softest-A unforced structured output**. Decisive merits: superset MCP client, real subagents/forks/recipes/skills, multi-model+self-host, prompt caching, **Linux-Foundation governance** (beats all single-maintainer options).
3. **opencode/MiMo** — complete TS harness, in-process server, tool-calls + emulated structured output (~9/13), subagents, `forkSession`, MCP client. Below Goose: **TS sidecar** + single-org bus-factor vs Block/LF.
4. **Pi / oh-my-pi** — round-3 disqualified in vanilla (no MCP/subagents/structured-output); fixes only in single-maintainer fork. Below the others on capability + bus-factor.
5. **Hermes-pattern** — not a Rust harness; valuable as the **design reference** for tool-call-as-control + `[SILENT]` sentinel + coerce/repair-never-reject. Goose independently validates this posture.

**Deciding factor:** *Do you want to OWN the loop or DELEGATE it?* RightClaw has **already built** worker/cron/reflection/learning around a thin invocation contract. rig's embed model fits that. Goose's value peaks only if RightClaw were willing to **throw away its hand-built loop** and adopt Goose's recipe/subagent/session machinery wholesale as a subprocess — which contradicts the current design. So Goose does **not** dethrone rig.

**The two make-or-break facts, stated plainly:**
- **(a) Is the Goose agent loop embeddable as a Rust crate, or app/server-only?** Technically callable in-process (`Agent::reply` is `pub`), but **not as a stable artifact**: unpublished (crates.io `goose` is an unrelated load-tester), edition 2021, `Config::global()`×171 process-global singleton, and Block's sanctioned surface is a separate process (ACP/goosed; the uniffi SDK is a ping→pong stub with no `goose`-core dep). **Effectively app/server-only for production adoption.**
- **(b) Does its MCP client do remote StreamableHTTP + Bearer?** **Yes — and it is a superset of RightClaw's need.** `ExtensionConfig::StreamableHttp` with `custom_headers` (tested `Bearer ${AUTH_TOKEN}`) + OAuth + UDS, on rmcp 1.4. RightClaw's `:8100/mcp` Bearer aggregator attaches verbatim.

---

## 7. Recommended next step

**Primary recommendation: embed rig; harvest Goose as a reference.** Specifically reuse, regardless of harness choice:
- `final_output_tool.rs` — synthetic-tool + local jsonschema validation + bounded continuation re-prompt pattern.
- `providers/formats/anthropic.rs` (the **`goose` crate** copy) — `cache_control` placement (last + second-to-last user msg, last tool-spec, system prompt).
- `providers/toolshim.rs` — tolerant JSON parser + interpreter-model extraction for weak models.
- `agents/extension_manager.rs` — the cleanest worked example of **rmcp 1.x remote StreamableHTTP + Bearer + OAuth + UDS** (RightClaw is on 1.7; patterns transfer).
- `skills/mod.rs` — agentskills.io SKILL.md discovery + validation (incl. `.claude/skills`); `subagent_handler.rs` — isolated sub-Agent with structured return.

**Only if you want to seriously contest rig — run ONE spike** before committing: drive a `goose acp` (or goosed-HTTP) subprocess as RightClaw's harness with the **RightClaw MCP aggregator attached as a StreamableHttp extension**, and measure:
1. Per-turn integration cost + ACP wire-contract stability across releases (agent-client-protocol 0.11 + goose custom_requests; v1→v2-rc churn).
2. Prompt-caching control, **budget/turn caps + reflection** equivalence, NDJSON-equivalent logging.
3. Whether reply-envelope / cron-delivery / prefilter-classify signals map cleanly onto Goose tool-calls + per-recipe `final_output`.
4. rmcp 1.4 (Goose) ↔ rmcp 1.7 (RightClaw server) wire interop.

(Two former spike items are now **resolved from source** and dropped: the `final_output` continuation loop **is** bounded by `max_turns`; ACP `fork_session` **is** dispatched via `on_fork_session`.)

If that spike shows the ACP drive is stable and cheap, Goose becomes a coin-flip with rig. If it shows protocol friction or the `Config::global()` singleton leaking per-agent state, **rig wins outright** and Goose stays a design quarry.

**Key UNKNOWNs to resolve in the spike:** top-level budget/reflection surface; ClawHub-vs-agentskills.io field-level SKILL.md delta (esp. `allowed-tools`, registry/install conventions); `response_format` passthrough constrained-decoding behavior on vLLM; effort to neuter `Config::global()` if embedding is ever reconsidered; whether the `claude_code` provider preserves RightClaw's `claude -p` invariant flags.
