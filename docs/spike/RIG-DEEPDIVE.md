# RIG DEEP-DIVE — verified due-diligence on the #1 pick (2026-06-19)

Pressure-tests rig (0xPlaygrounds/rig) the same way mimo was examined, against **current source** (clone @ HEAD `57de2005`, release **v0.39.0**, 2026-06-20) + GitHub issues/PRs (open & closed). Supersedes the stale rig claims in rounds 1–9 of `harness-migration-research.md`. Every claim cited to `crates/rig-core/...:line` or issue/PR number. Method: 3 parallel verification subagents (core-fit / MCP+providers+caching+streaming / maturity+state+linking).

## Verdict in one line

The capability question is **settled green** — rig delivers byte-prompt-control + MCP(rmcp) + prompt-caching + native structured output + 24 providers + static-musl. The rig decision reduces to ONE tradeoff: **absorb HIGH upstream churn + own the glue (subagents/fork/skill-loader + ~3 small patches) for full control + minimal surface + Rust in-process.** Under the user's priorities (minimalism+control, cost-no-object, willing to own code) → **rig #1 holds, now on verified ground.**

## CONFIRMED — the claims that justified rig #1

| Claim | Verdict | Evidence |
|---|---|---|
| **Byte-exact system prompt** (the thing mimo structurally can't do) | **CONFIRMED** | Preamble → `Message::system(preamble)` prepended verbatim (`agent/completion.rs:135-141`); emitted verbatim by Anthropic (`anthropic/completion.rs:2301-2312`), OpenAI Chat (`openai/completion/mod.rs:1237`), OpenAI Responses (`responses_api/mod.rs:860-861`). Tool descriptions = separate structured `tools` field, **never** system text (`completion/request.rs:544`; anthropic comment 155-156 "independently of the system prompt"). RAG docs → a separate `Message::User`, not system (`request.rs:575-611`). **No always-on injected text**; `ConversationMemory` is opt-in and injects no instructions (`agent/completion.rs:299-302`). |
| **MCP client → attach `:8100/mcp` with per-agent Bearer** | **CONFIRMED** | `McpClientHandler` + `McpTool` (`tool/rmcp.rs`, PR #1525); builders `.rmcp_tools(...)` (`agent/builder.rs:328,422-472`). Transport = any rmcp transport incl. `StreamableHttpClientTransport`; rmcp 1.7 `StreamableHttpClientTransportConfig::with_uri(url).auth_header(token).custom_headers(...)` applies `bearer_auth` on every request. Bug #1914 (StreamableHttp re-init drops in-flight response) **fixed** by #1921 (+300s tool timeout). Gaps non-blocking: #1475 (MCP *resources* unsupported — tools-only, fine for RightClaw), #1906 (typed registry, proposed). |
| **Prompt caching (Anthropic `cache_control`)** | **CONFIRMED (first-class)** | Typed `CacheControl::Ephemeral{ttl}` + `.with_prompt_caching()` / `.with_automatic_caching()` / `_1h()` (`anthropic/completion.rs:159,180-205,1556-1611`); applied on **both** non-streaming + streaming (`anthropic/streaming.rs:67-97`); 4-marker budget enforced; reads back `cache_read/creation_input_tokens`. |
| **Multi-model / self-host** | **CONFIRMED** | 24 first-party providers (`providers/mod.rs:66-91`) incl. `anthropic, openai, moonshot, xiaomimimo(=mimo), zai, deepseek, together, openrouter, ollama, llamafile, mistral, groq, xai…`. OpenAI-compatible custom base_url (`client/mod.rs:650-658`, `OPENAI_BASE_URL`) for vLLM/Venice/OpenRouter. **`moonshot` is first-party → the non-Venice Kimi path** the whitelist work was blocked on. (Venice itself not first-party → OpenAI-compat path; quirks UNVERIFIED.) |
| **Static-musl single binary** (round-6 open spike) | **FEASIBLE** | rustls is the **default** (`rig-core/Cargo.toml: default=["reqwest","derive","rustls"]`; workspace `reqwest{default-features=false}`; #1682 "rustls by default for everything", native-tls opt-in). rmcp 1.7 shares reqwest 0.13 TLS — no second stack. **Caveat:** pin rustls crypto provider to `ring` (aws-lc-rs needs a C/CMake toolchain) and avoid gRPC/vertexai/fastembed companion crates that drag in aws-lc-rs/native-tls. No musl-breakage issues found; repo is wasm-aware. |

## Structured output + tools on the same turn — the nuanced one (CRITICAL for self-host)

RightClaw needs structured output AND MCP tools **every turn**. rig's behavior is split by code path:

- **Anthropic — COEXIST, no suppression.** `output_config.format.json_schema` built independently of `tools` (`anthropic/completion.rs:2323-2333`). **Native server-side schema → no synthetic-tool wrapping → no `propertyNames` injection.** (The whitelist's `propertyNames` failure was a mimo-emulated-forced-tool + Venice artifact; rig on Anthropic/OpenAI dodges it entirely — the "mechanism" axis resolves favorably.)
- **OpenAI Responses API (DEFAULT client) — COEXIST, no suppression.** `output_schema` → `text.format` applied unconditionally (`responses_api/mod.rs:935-948`).
- **OpenAI Chat Completions (opt-in client; the self-host/vLLM/Venice path) — SUPPRESSED on first tool-turn.** `should_apply_response_format = output_schema.is_some() && (tools.is_empty() || history_has_tool_result)` (`openai/completion/mod.rs:1282-1283`); introduced by **PR #1622 (merged, for llama.cpp #1604)** but fires for *all* chat-completions backends. `response_format` silently dropped on the initial tools+schema turn until a tool-result exists in history.
- **Systemic — #1928 (OPEN):** the agent loop reattaches `output_schema` on **every** iteration (`agent/prompt_request/mod.rs:566,617-629`), biasing the model to emit JSON instead of calling tools mid-loop. Maintainer fix = **PR #1929 (OPEN, unmerged)** adding `OutputMode` + `composes_native_output_with_tools()` (true for OpenAI both + Anthropic; false→synthetic-tool for Ollama/others). Per #1929 the maintainer judges Anthropic+OpenAI compose correctly; **Ollama native-but-unconditional breaks tools; Gemini errors; llamafile silently drops the schema** (`llamafile.rs:297-299` warn-and-discard).

**Implication:** clean on Anthropic + OpenAI-Responses (the hybrid-Claude + frontier path). On the **self-host OpenAI-compat path** you inherit first-turn suppression + #1928 every-turn-schema → carry the #1929 patch or accept the behavior. This is the one structured-output sharp edge for the multi-model goal.

## Sharpened RISKS (don't dethrone rig, but on the table)

- **Churn — HIGH (dominant cost).** `rig-core` ~biweekly minors (0.34→0.39 in ~11 weeks); `[breaking]` in **5 of last 8 minors** — 0.39.0 rewrote the agent loop into a sans-IO state machine (#1899) + changed tool registration (#1913); 0.37 changed `Chat` to mutate caller history (#1733); `max_depth`→`max_turns` (#1323). **≥10 breaking changes in ~11 versions. No 1.0 roadmap** (maintainer acknowledged the breakage treadmill in #628, Jul 2025; cadence continued). Budget a migration most bumps; treat the core API as not-yet-frozen.
- **Bus factor — MEDIUM-HIGH.** 0xPlaygrounds is a company (7694★, 217 contributors, **fast triage** — a real strength), but recent *core* throughput is ~80% one engineer (`gold-silver-copper`); all-time top maintainers (joshua-mo-143, cvauclair) now low-activity → turnover already happened.
- **Fail-fast — 2 deliberate silent stream-swallows** (violate the project's propagate-or-die rule until patched): `streaming.rs:441-443` converts any `ProviderError` whose message contains substring `"aborted"` → `Ok(None)` (brittle); `openai/completion/streaming.rs:171-178` (+`deepseek.rs:743`) logs-and-`Ok(None)` on a chunk deserialize failure — **the self-host/Venice path**. Both small + upstream-able. (Core stream architecture IS otherwise fail-fast: transport/SSE errors propagate, regression-tested.)
- **State/forks — caller-owned, no native fork.** State = `Vec<Message>` the caller owns (`completion/message.rs:31`, `completion/request.rs:277`). Opt-in `ConversationMemory` is a flat in-process buffer, **not durable**, no session-tree (`memory.rs:85,348`). Fork = clone the vec. Fine — RightClaw owns its own session/`data.db`.
- **Subagents — none native.** Compose via agent-as-tool (`impl Tool for Agent`, `agent/tool.rs:16`) + `pipeline` combinators. Build-your-own (expected; RightClaw's subagent model is its own anyway).
- **Skills — none** (separate finding, FINAL-RANKING 2026-06-19): build a small `tool`-based SKILL.md loader; learning pipeline already RightClaw's.
- **Top open papercuts:** #446 (some providers silently drop `Document`/`ToolResult` content — Ollama/Anthropic), #1538 (`metadata:null` deserialize break on OpenAI-compat vLLM/glm), #1098 (not all streams return aggregated final text), #1250/#1820 (awkward access to per-turn messages + raw provider errors). #1930 = SQLi + broken SQL in `rig-postgres`/`rig-lancedb` — irrelevant unless you use those first-party vector stores (RightClaw uses Turso).

## Actionable connections to other workstreams

1. **Kimi (whitelist TODO #1, was blocked on a non-Venice provider):** rig ships a first-party `moonshot` provider → adopting rig gives you the non-Venice Kimi path natively. Test Kimi via rig+Moonshot with native `response_format` to close "Venice-tooling vs Kimi-can't" — and it avoids the `propertyNames` artifact by construction.
2. **`propertyNames` is moot on the rig major-provider path** — native schema on Anthropic/OpenAI, no synthetic-tool wrapping. The whitelist's dominant `(model×provider×mechanism)` caveat resolves favorably for rig except on the OpenAI-compat self-host path.
3. **The 3 patches RightClaw would carry/upstream:** PR #1929 backport (self-host structured-output+tools), the 2 stream-swallow fixes. All small; all upstream-friendly.

## Pre-commit checklist if adopting rig (replaces the stale round-6 spikes)
- [ ] Confirm native `response_format`+tools on your actual self-host model set (vLLM) via the chat-completions client WITH #1929 applied (or accept first-turn suppression).
- [ ] Static-musl build of `rig`(rmcp,rustls/ring) + reqwest + tokio — verify no aws-lc-rs/native-tls in the tree.
- [ ] Patch the 2 stream-swallow sites for FAIL-FAST; upstream them.
- [ ] Pin a rig-core version and write the migration-on-bump runbook (churn is HIGH).
- [ ] Build the SKILL.md `tool`-loader + agent-as-tool subagent shim + clone-vec fork; port worker/cron/reflection orchestration onto rig's `AgentRun`.
