# OMP + PI Deep-Dive for RightClaw — Round 13 (Empirical + Current-Source)

*2026-07-18. Status: capstone-grade. Supersedes the omp/pi rows in FINAL-RANKING.md and extends PI-REEVAL.md (round 11) with (a) current-source deep dives at omp v17.0.4 and pi 0.80.10, and (b) EMPIRICAL probes run against a locally installed omp 17.0.0 driving real headless turns, a real MCP round-trip, session resume/fork, and prompt-footprint measurements. Every claim is marked **[E]=empirical (ran it)**, **[S]=source-verified (path cited)**, **[NPM/GH]=registry/API**, or **UNKNOWN**.*

> **Round 14 addendum (same day, later): pi+extensions re-rates UP.** Two community packages eliminate round 13's "you build the MCP adapter / task tool" cost for pi — see §9. New empirical results: S2 setup-token PASS (omp+pi), S1 mechanism-C structured output PASS (omp 18/18; pi terminate:true cleaner), pi+pi-mcp-adapter vs the REAL aggregator PASS (omp still has an open MCP-registration issue in fresh profiles). **Verdict shifted: pi+extensions is now co-equal-to-leading vs omp.**
>
> **Round 15 addendum (same day, evening): the omp "MCP bug" is SOLVED (not a bug), S3 is GREEN, scope mechanism mapped.** (1) RightClaw's invocation-scope binding is a **per-request HTTP header** baked into the bot-written per-invocation mcp.json — subagents sharing the config inherit parent scope correctly; no aggregator change needed. (2) omp's "fresh-profile MCP failure" = **`NULL_PROMPT=true` wipes the xd:// tool inventory that MCP tools are exposed through when `tools.xdev=true`** (default); fix = `tools.xdev: false` (native direct tools, prompt-independent). (3) With the fix, **full sandbox E2E PASS**: all 43 aggregator tools registered in-sandbox, live `chat_search` call returned the correct scope error. (4) User-weighted criteria (subagents+IRC, token efficiency, programmable access, forks, auto-skills) favor **omp 3/5 with parity on forks; auto-skills moot vs RightClaw's own learning pipeline**. See §10.

---

## 0. Headline

**omp 17 is a credible `claude -p` replacement TODAY for RightClaw's per-turn sandbox harness — empirically, not on paper.** Every hard requirement of the Claude Invocation Contract has a verified omp path except structured output (`--json-schema`), which maps onto omp's mechanism-C (`yield` tool + `session.outputSchema`, or `task` `outputSchema`) — the exact direction round 11–12 already converged on. Vanilla pi 0.80.x is the same per-turn engine minus MCP/subagents with **better session flags and better governance**, but requires building the MCP adapter.

This creates a **fourth option in the fork**, sitting between Goose (adopt, cede prompt control) and rig (embed, own everything): **adopt a complete TS harness that DOES give byte-exact prompt control (omp, verified) or near-exact (pi), runs as the same sandbox-subprocess-over-SSH shape as today, and keeps skills/subagents/forks/MCP.**

---

## 1. Method

1. **Empirical (omp 17.0.0, nix store binary, macOS host):** real headless turns with NDJSON capture; session resume with cache measurement; `--fork`; `--session-id` attempt; minimal-prompt footprint (3 configurations); `.claude/skills` drop-in; end-to-end MCP against a purpose-built dummy streamable-HTTP server logging auth headers and JSON-RPC methods (`docs/spike/harness/omp_mcp_probe.py`); MCP tool-naming under xdev on/off; `NULL_PROMPT` byte-exactness; server-key naming-trick attempt. Raw artifacts under `/tmp/omp-probe/` (ephemeral).
2. **Source deep-dives (two parallel scouts):** omp cloned @ `3fdd85ab` (== tag `v17.0.4`; npm latest 17.0.4 published ~5h before research [NPM]); pi @ `3da591ab` (2026-07-17; npm `@earendil-works/pi-coding-agent@0.80.10` [NPM]). Full reports: `/tmp/omp-deepdive-report.md` (441 lines) and agent artifact (pi). Local install is omp 17.0.0; one patch behind — no behavior difference observed vs the 17.0.4 source for anything cited here.
3. **Contract extraction:** `crates/bot/src/cc/invocation.rs:502-617` (`into_args`), `crates/bot/src/cc/prompt.rs:289-291` (`--system-prompt-file`), `crates/bot/src/cc/stream.rs:71-128,326-545` (what the bot parses).

---

## 2. The RightClaw contract, mapped (the table that matters)

RightClaw today (`invocation.rs`): `claude -p --dangerously-skip-permissions --mcp-config <p> --strict-mcp-config [--allowedTools/--disallowedTools|--tools ""] [--resume id|--session-id id [--fork-session]] [--model m] [--max-budget-usd b] [--max-turns n] [--debug --debug-file] --output-format stream-json --verbose --json-schema <s> --system-prompt-file <p> -- <prompt>`, over SSH, fail-closed.

| Contract element | omp 17 | pi 0.80 |
|---|---|---|
| One-shot per-turn process | ✅ `-p` (or piped stdin auto-activates print mode) [E][S `main.ts:1155-1159`] | ✅ identical [S `main.ts` resolveAppMode] |
| NDJSON event stream | ✅ `--mode json`: `session` header + `agent_start/turn_start/message_*/tool_execution_*/turn_end/agent_end`, one event/line [E][S `print-mode.ts:84-112`] | ✅ `--mode json`, same taxonomy family [S `print-mode.ts`, `docs/json.md`] |
| Long-lived control channel | `--mode rpc` (full JSON-RPC 2.0), `rpc-ui`, `acp` [S] | `--mode rpc` (never one-shot; its orchestrator pkg keeps 1 proc/instance) [S] |
| Exit codes | 0 success; 1 on arg error (verified) / turn error [E][S `print-mode.ts`] | 1 on assistant stopReason error/aborted [S `print-mode.ts`] |
| Skip permissions | ✅ `--auto-approve` / `approval-mode: yolo` (plus `--no-tools`) [S] | ✅ no permission system by design [S] |
| Session resume by id | ✅ `--resume <uuid>` verified; cacheRead 31,744/31,984 tokens on 2nd turn (99% hit) [E] | ✅ `--resume/--session` [S] |
| **Pre-seed new session id** | ❌ **NO `--session-id`** (probe: "unknown flag"). Workaround: first turn mints uuidv7, emitted in the `session` header event immediately → bot captures+persist it. Small orchestration change, arguably cleaner than pre-seeding. [E][S §2] | ✅ **`--session-id <id>` create-if-missing**, and works with `--no-session` for deterministic cache-affinity ids (their #6070 — RightClaw's exact use case) [S `args.ts`, CHANGELOG] |
| Session fork | ✅ `--fork <uuid>` → new session id, full parent context (verified: recalled "OK") [E]. Hidden from `--help` but functional; requires persistence. [E][S `session-manager.ts:1909-1914`] | ✅ `--fork` + `SessionManager.forkFrom` + in-place tree branch API (stronger) [S] |
| Session storage | JSONL `~/.omp/agent/sessions/<mangled-cwd>/<ts>_<uuid7>.jsonl`; subagent children under `<uuid>/` dir; `PI_CODING_AGENT_DIR` relocates; `--session-dir`; `--no-session` in-memory [E][S] | JSONL `~/.pi/agent/sessions/...`; `PI_CODING_AGENT_DIR`; `--session-dir`; `--no-session` [S] |
| System prompt replacement | ✅ `--system-prompt` = full template replace [E][S `system-prompt.ts:446,817`]. Residual footer (env/cwd/workspace/tool inventory) ≈ 1.4k tokens with all discovery off [E]. **Byte-exact: `NULL_PROMPT=true` + `--append-system-prompt` → 89 input tokens total** (vs 31,886 default) [E] | ✅ `--system-prompt` full replace, BUT cwd line + `<project_context>` + `<available_skills>` XML always appended; no NULL_PROMPT found. Small residue, no byte-exact path. No datetime injection (removed for cache stability, #6621). [S `core/system-prompt.ts`] |
| Composite prompt sections (RightClaw assembles chat context, MCP section, focus, memory markers into stdin/prompt) | Unaffected — prompt passed as argv/stdin; `--` separator not needed, positional args + stdin both work [E] | Same [S] |
| Tool gating | `--no-tools` (all off), `--tools <allowlist>` (builtin names only; verified list includes `task, hub, todo, browser, memory_edit, retain, recall, reflect, learn, manage_skill, yield, goal…`) [E]; config `disabledProviders` for discovery sources [S] | `--no-tools`, `--no-builtin-tools`, `--tools`, `--exclude-tools` (finer granularity) [S] |
| **MCP: config file** | ✅ **project-root `.mcp.json` (claude-compatible!) discovered with cwd=/sandbox — verified end-to-end** [E]. Also `~/.omp/agent/mcp.json`, `<cwd>/.omp/mcp.json`; `mcp.enableProjectConfig` gates project discovery [E][S `dirs.ts:836-842`; binary strings]. ❌ no `--mcp-config`/`--strict-mcp-config` flags; `--config <overlay.yml>` is settings-level not MCP-specific [S §7]. RightClaw already writes `/sandbox/.mcp.json` → **discovery fit is exact**. | ❌ **No MCP at all, by design** (zero mcp paths repo-wide). Extension API `pi.registerTool()` + lifecycle hooks is the blessed build path [S §7]. |
| **MCP: transport + auth** | ✅ streamable-HTTP verified: initialize → `Mcp-Session-Id` → GET SSE stream → tools/list → tools/call, **`Authorization: Bearer <token>` on every request** from `.mcp.json` `headers` map [E]. stdio + SSE(deprecated) also; full OAuth 2.1 flow; `OMP_MCP_TIMEOUT_MS`; background connect doesn't block startup [S `mcp/transports/*`] | — (build adapter) |
| **MCP: tool naming** | ⚠️ **`mcp__<server>_<tool>`** — single underscore (`mcp__right_probe_ping`), prefix-dedup + sanitize [E both xdev on and off; S `mcp/tool-bridge.ts:273-298`]. Server-key `right_` trick normalized away [E]. **RightClaw's `mcp__right__*` (double underscore) does NOT carry over** → mechanical rename across `skills/`, `templates/right/`, `crates/right-agent/src/codegen/agent_def.rs`, `PROMPT_SYSTEM.md`, `with_instructions()` ×2 (the rename surface ARCHITECTURE.md already enumerates). | — (RightClaw defines names in its own adapter → can keep `mcp__right__*`) |
| **MCP: health signal** | ❌ **Dead server = silent.** Turn succeeds exit 0, no event-stream signal, no init event [E]. claude's `system/init.mcp_servers[]` (which `stream.rs:1003-1063` health-checks) has no omp equivalent → RightClaw must redesign MCP-health gating (bot-side probe of aggregator, or require/verify first-turn MCP tool call). | — |
| **Structured output** | ❌ no `--json-schema`/response_format anywhere (exhaustive grep) [S §4]. ✅ mechanism-C: `yield` builtin validates payload against `session.outputSchema` (AJV), `task` tool `outputSchema` per subagent (`schemaMode: permissive\|strict`) [S `tools/yield.ts:240-244`, `task/types.ts:112-235`]. No CLI flag sets `session.outputSchema` → ship a tiny `-e` extension reading schema path from env. **Spike item S1.** | ❌ same absence; canonical `defineTool … terminate:true` example [S `examples/extensions/structured-output.ts`] |
| Model per invocation | `--model` (fuzzy), role flags `--smol/--slow/--plan`, env `PI_*_MODEL` [S] | `--model provider/id[:thinking]`, `--provider` [S] |
| Max turns | ❌ no flag. `--max-time` exists; bot can kill SSH proc or extension enforces via loop callback. Gap (cron uses `--max-turns`). [E help] | ❌ no flag (unbounded `while(true)`; `shouldStopAfterTurn` callback is the SDK lever) [S `agent-loop.ts`] |
| Max budget USD | ❌ no flag. omp reports usage w/ cost (0 for OAuth accts [E]); bot-side budget gate already exists for learning — extend to turns. | ❌ same |
| Anthropic auth | ✅ `ANTHROPIC_OAUTH_TOKEN` (precedence) / `ANTHROPIC_API_KEY` env [S `pi-ai/registry/anthropic.ts:10-13`]. RightClaw renames its injected `CLAUDE_CODE_OAUTH_TOKEN` → one-line change. **Whether setup-token (claude.ai OAuth) tokens work on omp's Anthropic wire = UNKNOWN → spike item S2.** | ✅ same env vars + first-party Pro/Max OAuth flow [S `providers/anthropic.ts`] |
| Multi-model / self-host | ✅ 36 providers incl native `kimi.ts` (Kimi Code, `kimiApiFormat: openai\|anthropic`), Synthetic (OpenAI-compat shim), per-model `baseUrl` [S] | ✅ 36 providers incl `moonshotai(+cn)`, **`kimi-coding` (Anthropic-shaped, api.kimi.com/coding, KIMI_API_KEY)** [S `providers/all.ts`, `kimi-coding.ts`] |
| **Kimi non-Venice path (whitelist TODO #1)** | ✅ native Kimi Code provider [S] | ✅ `kimi-coding` + `moonshotai` [S]. **Either harness finally measures Kimi's true structured-output ability without Venice.** |
| Weak-model tool-call support | ✅ **`tools.format` dialects**: `auto\|native\|glm\|hermes\|kimi\|xml\|anthropic\|deepseek\|harmony\|qwen3\|gemini\|gemma\|minimax` — tool-call format emulation per model family [E config list; S settings-schema]. Directly addresses whitelist fragility. | ⚠️ per-provider compat knobs only (no dialect emulation layer found) [S] |
| Skills | ✅ SKILL.md; frontmatter `{name,description,globs,alwaysApply,hide,disableModelInvocation,+passthrough}`; discovery: `.omp/skills` walkup, **`~/.claude/skills` + project `.claude/skills` walkup** (project probe: skill followed → "SKILL-LOADED-CONFIRMED" [E]), `.codex`, opencode dirs, Claude marketplace plugins; `--skills` globs, `--no-skills` [E][S `discovery/*.ts`] | ✅ SKILL.md (agentskills.io); `.pi/skills`, `.agents/skills`, `--skill <path>` (repeatable, additive even with `--no-skills`); `.claude` via one-line settings opt-in or `--skill /sandbox/.claude/skills` per invocation [S `docs/skills.md`] |
| Subagents | ✅ builtin `task` tool: frontmatter `{name,description,tools,spawns,model,thinkingLevel,blocking,prewalk}`, bundled agents (scout/reviewer/task/sonic…), sync/async/batch `tasks[]`, isolated settings snapshot, fresh child prompt, own JSONL transcript, worktree isolation, `spawns` allowlist + `taskDepth` tracking, 500KB/5000-line caps, `outputSchema` structured return [S `task/*`; E saw child sessions on disk]. **Note: RightClaw's bot orchestrates subagent-like flows itself (probe-writer, curator, delivery) — a harness task tool is a bonus, not a requirement.** | ❌ no builtin (7-tool closed enum); example extension spawns `pi --mode json -p --no-session` subprocess per task — which IS RightClaw's per-turn shape anyway [S `examples/extensions/subagent/`] |
| Context forks (probe-writer, background continuation) | ✅ `--fork` verified [E] | ✅ `--fork` + session-tree API [S] |
| Compaction | ✅ auto (snapcompact default, mid-turn) + manual `/compact`, idle-compaction knobs [S] | ✅ manual + auto [S] |
| Deployment in sandbox | Single bun-compiled binary, **linux-x64 165.8MB / arm64 142.1MB, glibc ≥2.17, NO musl** [GH releases][S ci.yml:33-40]. `pi_natives` N-API embedded (mandatory, in-binary). No daemon needed for `-p` [S §9]. Upload to `/sandbox/.local/bin/omp` via existing SSH+tar path; verify checksum (known upload bug is small-files-in-dirs, single big file is safe). | Single binary too, **much smaller: ~43MB compressed linux** [GH release]; 6 platforms via `bun build --compile` on every tag [S build-binaries.yml]. |
| Learning pipeline hooks | `autolearn/*` + `learn`/`manage_skill` builtins exist (omp's own skill learning — must be disabled/ignored; RightClaw's pipeline writes `rightx-*` SKILL.md files which omp picks up via `.claude/skills` [E]) | None — clean slate for RightClaw's pipeline |
| Memory collision | `mnemopi` = omp's memory engine, **`memory.backend: "off"` by default** [S settings-schema:2451-2454] → the round-11 "mnemopi collides with Hindsight" concern is a non-issue: ship it off. Also a `hindsight.*` config section exists in 17.0 (`hindsight.retainEveryNTurns` etc.) [E config list] — possible native Hindsight integration, UNKNOWN detail, worth a look since RightClaw's memory IS Hindsight Cloud. | None |

---

## 3. Empirical log (what I actually ran — omp 17.0.0)

| Probe | Result |
|---|---|
| `omp -p --mode json "Reply with exactly: OK"` | NDJSON: `session`(uuidv7, cwd) → `agent_start` → `turn_start` → `message_*` → `turn_end`(toolResults, usage, stopReason) → `agent_end`. Exit 0, stderr clean. Input **31,886 tok** (default prompt+tools heavy). |
| `--resume <uuid>` follow-up | Same session id, recalled "OK", **cacheRead 31,744 / total 31,984** — prompt caching works across invocations. |
| `--session-id <uuid>` | **Error: unknown flag** — no pre-seed. |
| `--fork <uuid>` | New session id emitted; recalled parent content ("OK"). Works; hidden from `--help`; refuses `--no-session`. |
| `--no-tools --no-skills --no-rules --no-extensions --no-lsp --system-prompt "…calculator…"` | Input **1,474 tok** — residual = project footer (env/cwd/workspace). |
| `NULL_PROMPT=true --append-system-prompt "…calculator…"` (+ above flags) | Input **89 tok** — **byte-exact prompt control confirmed**. |
| `.claude/skills/probe-skill/SKILL.md` in cwd, default config | Skill discovered + followed ("SKILL-LOADED-CONFIRMED"). RightClaw's `/sandbox/.claude/skills/rightx-*` pattern drops in. |
| `.mcp.json` `{right: {type:http, url, headers:{Authorization: Bearer probe-token}}}` + dummy server | Full handshake logged server-side: initialize → initialized → tools/list → tools/call, **Bearer on every request**; tool executed, `pong:hello` returned to model. |
| MCP tool name (xdev on AND off) | `mcp__right_probe_ping` both modes. xdev on: model drives it via `xd://mcp__right_probe_ping` read/write; xdev off (`--config overlay.yml tools.xdev=false`): direct first-class tool call. |
| Server key `right_` naming trick | Normalized to `mcp__right_probe_ping` — sanitizer strips; no claude-style `__` obtainable. |
| Dead MCP server (port closed) | Turn completes normally, exit 0, **zero MCP signal in event stream**. |
| **S2: setup-token auth (2026-07-18, agent `agent-b` token, `sk-ant-oat01-…`)** | **PASS both.** Direct curl control: HTTP 429 (rate-limited but AUTHENTICATED — 401/403 would be the failure). omp: isolated `PI_CODING_AGENT_DIR`, `ANTHROPIC_OAUTH_TOKEN=$TOKEN`, `--model anthropic/claude-sonnet-5` → `provider:anthropic model:claude-sonnet-5`, `stopReason:stop`, real USD cost fields, correct calculator answer, 38 input tokens. pi 0.80.5 (bunx, same env var): printed `OK`. Neither persisted the token (grep of profile dir: no `sk-ant`). **The path-destroying risk is retired: RightClaw per-agent setup-tokens work in both harnesses via env rename `CLAUDE_CODE_OAUTH_TOKEN`→`ANTHROPIC_OAUTH_TOKEN`.** |
| stdin hygiene | omp merges piped stdin with argv prompt (`readPipedInput()`); a contaminated stdin (harness notices leaked into it) became part of the user message. **Bot invocations of omp MUST pin stdin explicitly** (message via stdin + `/dev/null`-equivalent control, or argv + closed stdin). Same discipline as today. |
| `--tools "mcp"` | Rejected: `--tools` allowlists builtin names only. Valid list exposed: read, bash, edit, ast_grep, ast_edit, ask, debug, eval, github, glob, grep, lsp, inspect_image, browser, checkpoint, rewind, task, hub, todo, web_search, write, memory_edit, retain, recall, reflect, learn, manage_skill, yield, goal. |
| `omp config list` | 438 keys. Notables: `tools.format` dialects, `mnemopi.*`, `hindsight.*`, `autolearn.*`, `task.disabledAgents`, `skills.enableClaudeUser`, `mcp.enableProjectConfig`, `compaction.*`, `modelRoles`. |

---

## 4. Governance / churn (the real cost axis)

| | omp | pi |
|---|---|---|
| Age | 6.5 months (created 2025-12-31) [GH] | pi-mono lineage, renamed earendil-works/pi (same repo id) |
| Commits/authors | can1357 **8,297** all-time, every release PGP-signed by agent-b; #2 `roboomp` 2,107 (circumstantially a bot: Dockerfile.robomp, mechanical PR titles); tail of 448 contributors [GH] | Zechner + **Ronacher (mitsuhiko)** + Brailovsky core; broad external credits in CHANGELOG (@Jaaneek @petrroll @markphelps …) [GH] |
| Cadence | majors **~monthly** (v15→16 = 33d, 16→17 = 30d); patches multiple/day (17.0.0→17.0.4 in 3 days) [GH] | 27 releases in 71 days (~1 per 2.6d), all lockstep minors [NPM] |
| Version policy | **None formal**; version scheme itself has reset repeatedly (1.337.x → …1337 schemes → 15/16/17.x); 552 npm revisions [NPM] | **Explicit**: "patch = fixes+additions, minor = breaking changes. No major releases." Per-package CHANGELOGs with Breaking Changes sections [S AGENTS.md] |
| npm | `@oh-my-pi/*@17.0.4`, bin `omp` compiled; **library entry = TS source** (`main: ./src/index.ts`) — library embedding needs Bun/build | `@earendil-works/*@0.80.10`, bin `pi` = **compiled dist/cli.js** (Node ≥22.19 or bun); clean SDK surface |
| Bus factor | **HIGH (1 human)** | MED (2–3 core) |

RightClaw adoption implication: **pin an exact omp version, vendor the binary, treat upgrades as deliberate events** (same posture as OpenShell alpha). pi's policy makes upgrades more predictable, but you carry the MCP adapter.

---

## 5. Where this lands the fork

Original 3-way: rig (control) / Option C (models) / Goose (middle). **omp/pi is a fourth path** that captures most of Goose's "ships everything" with dramatically better prompt control, and most of rig's "multi-model + hackable" without building the engine.

| | Option C (keep claude) | Goose | **omp** | **pi** | rig |
|---|---|---|---|---|---|
| Harness control | none | partial (minijinja + forced extras) | **byte-exact** (NULL_PROMPT) [E] | near-exact (small residue) | byte-exact |
| Structured output | native `--json-schema` | prompt-injected directive | mechanism-C port needed | mechanism-C port needed | native (Anthropic/OpenAI-Responses); chat-completions needs #1929 |
| MCP vs `:8100`+Bearer | native | native | **verified E2E** [E] | build adapter (~small) | rmcp wiring |
| Skills `.claude` drop-in | native | native-ish | **verified** [E] | one-line opt-in / `--skill` |
| Subagents/forks | native | native | builtin task + `--fork` [E] | build task tool (or bot-orchestrate); forks native |
| Multi-model/self-host | via backend swap | 20+ providers | 36 + dialects + kimi-coding | 36 + kimi-coding | 24 + moonshot |
| Runtime in sandbox | closed binary | musl static 38MB | glibc binary 166MB | binary ~43MB+ | static musl runner YOU build |
| Loop ownership | none | none | source TS, hackable; loop internals ceded | same | **full** |
| Churn | zero (pinned CC) | moderate | **HIGH (monthly majors, 1 maintainer)** | moderate (explicit policy) | HIGH (pre-1.0, no roadmap) |
| Effort to first green turn | none | small | **small** (probes prove it) | medium (MCP adapter) | large |

**Honest trade-off statement:** omp gets a sandbox turn green fastest — today, empirically — at the price of (1) mechanism-C structured-output port, (2) MCP-health redesign, (3) `mcp__right__*`→`mcp__right_*` rename (or adapter shim inside the aggregator: expose tool aliases? no — the rename is cleaner), (4) no `--max-turns`/`--max-budget` (bot enforces), (5) single-maintainer + monthly-major churn absorbed via pinning, (6) 166MB glibc binary in every sandbox. pi trades (3) away and halves the binary, at the price of building the MCP adapter and either building a task tool or continuing bot-side orchestration (which RightClaw already does).

**What omp/pi do NOT give over claude:** the model-facing loop polish (claude's agent loop is extremely tuned), `--json-schema` native enforcement, `system/init` health introspection, `Skill`/`Agent` tool UX the bot's stream parser already speaks (parser rewrite to omp's event taxonomy is required either way — `stream.rs` maps: `result`→`turn_end`/`agent_end`, `assistant`/`user` blocks→`message_*`/`tool_execution_*`, `system/init`→`session` header minus MCP status).

---

## 6. The spike that remains (omp path — do these IN ORDER)

- ~~**S2. Setup-token auth.**~~ **DONE 2026-07-18 — PASS (see §3).** `ANTHROPIC_OAUTH_TOKEN` + agent setup-token works for both omp and pi; env-only, nothing persisted.
- **S1. Mechanism-C structured output on RightClaw schemas.** Ship `-e` extension: read `RIGHT_OUTPUT_SCHEMA_PATH` env → `session.outputSchema = <schema>`; verify `yield`-enforced turns against the hard cron `oneOf{notify|silent}+run_note` and prefilter schemas, on Claude AND on weak families (Qwen3-235B, GLM-5, DeepSeek-v3.2 via OpenAI-compat `baseUrl`, with matching `tools.format` dialect). Measure: schema-valid first-try rate; does validation failure surface as an observable `isError` tool result (for abort-after-3 re-keying); cache behavior.
- **S3. Sandbox turn E2E.** Upload `omp-linux-x64` to a TestSandbox, glibc check, run over the existing SSH slot against the real aggregator `http://host.openshell.internal:8100/mcp` with a real agent Bearer token; verify `mcp__right_send_message` flows, session files under `/sandbox`, policy egress covers any omp phone-home (disable: `startup.checkUpdate=false`, share/my.omp.sh endpoints).
- **S4. MCP-health redesign.** Decide how the bot proves MCP liveness per turn without `system/init`: candidate = bot-side aggregator probe per turn (cheap, bot owns the aggregator) + treat "zero MCP tool definitions reached the model" as worker-side failure detectable via a turn-start marker. Must not regress `guard_no_sandboxed_host_exec` fail-closed posture.
- **S5. Kimi via `kimi-coding`/`kimi.ts` provider** (non-Venice) on the hard schemas → closes whitelist TODO #1.

(pi path = S1/S2/S3/S5 identical, plus **S0-pi: build the MCP adapter extension** — `pi.registerTool` bridge to a streamable-HTTP MCP client with Bearer; est. small, but it is real work and it is on the critical path.)

## 7. Explicit UNKNOWNs (do not bank)

1. ~~`ANTHROPIC_OAUTH_TOKEN` + setup-token compatibility~~ — **RESOLVED: PASS (§3).**
2. `session.outputSchema` settable via extension API in `-p` mode + `yield` UX in print mode (S1 mechanics inferred from `tools/yield.ts:240-244` + SDK surface, not run).
3. omp `hindsight.*` config section semantics — possible native Hindsight integration (RightClaw's memory backend!); unverified, potentially a significant alignment bonus or a collision.
4. Whether `--fork` (hidden) is stable API or transitional; pin-version posture mitigates.
5. omp daemon broker activation triggers in headless mode (believed optional/on-demand).
6. Subagent recursion hard cap (`taskDepth` threaded, max not located); `task.maxRecursionDepth: 3` seen in local config.yml.
7. glibc in OpenShell sandbox images (omp needs ≥2.17; claude runs there today so near-certain, but verify in S3).
8. pi: no NULL_PROMPT equivalent — the always-appended cwd line + skills XML is tolerable but not byte-exact; whether RightClaw cares depends on how S1 prompt assembly lands.
9. omp OAuth-cred storage in agent.db vs RightClaw's credentials-outside-sandbox invariant — for sandbox runs use env-var auth only, never `omp auth login` inside a sandbox.

## 8. Bottom line for the fork decision (SUPERSEDED by §9 — round 14)

If the weighting is "**adopt a harness, keep everything, multi-model, sandbox-drop-in, byte-exact prompt**", **omp is now the strongest adopt candidate — ahead of Goose** (byte-exact vs minijinja+forced-extras, verified MCP+skills+forks, bigger binary + worse governance are the costs). If the weighting is "**own the loop in Rust, minimal deps, absorb build cost**", rig's verdict is unchanged. **pi is the middle: omp's engine with better governance and smaller footprint, priced at one MCP adapter.** Option C remains the cheapest multi-model play and the fallback if S1 fails. **S2 (setup-token auth) is settled GREEN for both omp and pi — the subscription-credential path survives the migration.**

---

## 9. Round 14 — pi+extensions, S1/S2/S3 results, and the re-rate

### 9.1 The ecosystem correction (user-supplied, verified)

Round 13 priced pi as "build the MCP adapter + task tool". **Wrong — both exist as heavily-used community packages on the official pi.dev package index:**

- **`pi-mcp-adapter`** (nicobailon) v2.11.0 [NPM]: **132.6K dl/mo**, MIT. Reads claude-format `.mcp.json` (+`~/.config/mcp/mcp.json`, `$PI_CODING_AGENT_DIR/mcp.json`, `.pi/mcp.json`); headers with env interpolation; **Bearer + full OAuth incl. headless auth-start/auth-complete**; StreamableHTTP+SSE; lifecycle lazy/eager/keep-alive; `directTools` (individual tool registration from cache) or a ~200-token proxy tool; output guards. Forks already exist (`pi-tidy-mcp-adapter`, `@vllnt/pi-mcp`).
- **`pi-subagents`** (nicobailon) v0.35.1 [NPM]: **113.3K dl/mo**, MIT. Builtin agents (scout/worker/reviewer/oracle/planner…), chains+parallel+background runs with `status.json`/`events.jsonl` lifecycle artifacts, per-agent model overrides, watchdog. RightClaw note: the bot orchestrates subagent-like flows itself, so this is optional.

Governance note: both are effectively single-author (nicopreme) — but leaf, forkable components with a large user base, vs omp where the *whole harness* is the single-maintainer component.

### 9.2 S2 — setup-token auth: **PASS (omp + pi)**

Agent `agent-b` `sk-ant-oat01-` token as `ANTHROPIC_OAUTH_TOKEN`, isolated profiles: omp completed a claude-sonnet-5 turn (stopReason stop, real usage/cost, 38-token prompt); pi printed OK. Nothing persisted to disk. Direct-curl control: 429 (rate-limited but authenticated). **Subscription-credential model survives migration; change is the env rename from `CLAUDE_CODE_OAUTH_TOKEN`.** Also learned: omp merges piped stdin with argv prompt — bot must pin stdin explicitly.

### 9.3 S1 — mechanism-C structured output on RightClaw schemas: **PASS both, pi cleaner**

Extension registers `structured_output` with plain-JSON-Schema `parameters` (cron oneOf, prefilter); loop-level validation rejects invalid payloads back to the model as isError (natural retry).

- **omp (18 runs: claude-sonnet-5, kimi-code/k3, gpt-5.4-mini × cron-notify/cron-silent/prefilter ×2): 18/18 schema-valid, 17/18 first-try, 1 self-corrected retry; all semantically correct** (notify↔silent branches, create_new + `rightx-nginx-reverse-proxy` pattern). [E] Caveat: omp dropped generic `terminate` — after the accepted call the model emits a trailing text turn (harmless; bot ignores or suppresses). Kimi k3 (non-Venice) passes the hard oneOf — **S5 partially closed**.
- **pi (claude-sonnet-5 × cron-notify/cron-silent): 2/2 valid, `terminate:true` ends the loop — no trailing turn.** [E]

### 9.4 MCP vs the REAL aggregator (`127.0.0.1:8100`, agent-b's Bearer, 43 tools)

- **pi + pi-mcp-adapter: PASS.** Handshake + Bearer auth, full tools/list discovery (`right_chat_search`, `right_cron_*`…), live `tools/call` round-trip: `chat_search` → aggregator's server-side scope error `conversation_scope_unavailable` (CORRECT for an unregistered invocation — proves the full path). Tool naming `right_<tool>`. [E]
- **omp: OPEN ISSUE.** Against the real aggregator the handshake completes and tools/list returns 200 with all 43 tools (wire captured via logging proxy), but in **fresh/isolated agent dirs the tools never register to the model** (macOS and sandbox). In the default macOS profile (existing `~/.omp/agent`) project-scope `.mcp.json` works. Evidence points at omp's capability-discovery layer behaving differently in a fresh profile (user-scope MCP also ignores `PI_CODING_AGENT_DIR` — loads from the default agent dir). Root cause not yet pinned; next step is a focused read of `loadCapability` profile-dependent paths. [E]

### 9.5 S3 sandbox E2E (omp): deployment green, MCP blocked by §9.4

`omp-linux-arm64` uploaded to `test-sandbox-20260516-1649` (aarch64, glibc 2.39 ✓), sha256-verified, runs over the existing SSH slot, completes an Anthropic turn inside the sandbox with the setup-token. Aggregator reachable from sandbox (`401` without Bearer ✓). MCP registration blocked by the §9.4 fresh-profile issue. [E]

### 9.6 The re-rated fork

| | **pi + extensions** | omp |
|---|---|---|
| Per-turn NDJSON/resume/fork | ✅ (+ `--session-id` pre-seed) | ✅ (no `--session-id`) |
| Structured output | ✅ terminate:true, cleanest | ✅ works, trailing text |
| MCP vs aggregator | ✅ **verified E2E** | ⚠️ open registration issue |
| Skills `.claude` | one-line opt-in / `--skill` | ✅ native |
| Byte-exact prompt | ⚠️ residue (cwd line + skills XML; cache-stable, acceptable) | ✅ NULL_PROMPT |
| Binary | ~43MB | ~166MB |
| Governance | core team + explicit policy; extensions single-author but forkable | ❌ single maintainer, monthly majors |
| Weak-model dialects | per-provider compat | ✅ `tools.format` dialects |
| Build remaining | structured-output extension (~40 lines) + config layout + skills opt-in | nothing (once §9.4 closes) |

**pi + pi-mcp-adapter (+ optional pi-subagents) is now the leading adopt candidate**: every hard requirement is empirically green, the core is team-governed with an explicit breaking policy, and the two community extensions are popular, MIT, and forkable. omp remains the byte-exact/all-builtin alternative pending §9.4 and carries the highest governance risk. Remaining spikes: **S3-pi** (pi binary + adapter inside a real sandbox against the live aggregator — mirror of §9.5), **S4** (MCP-health redesign — both harnesses degrade silently; bot-side aggregator probe), **S6** (version-pin + vendor strategy: pi core + 2 extensions + omp are all fast-moving; decide pin/bump cadence).

> **§9.6 is SUPERSEDED by §10.5 (round 15):** the omp §9.4 "registration issue" was the NULL_PROMPT×xdev interaction (§10.2), not a bug — and with it fixed, omp's sandbox E2E is green (§10.3). The round-15 verdict flips the lead to **omp** on the user-weighted criteria.

---

## 10. Round 15 — scope mechanism, the NULL_PROMPT×xdev root cause, S3 green, criteria-weighted verdict

### 10.1 RightClaw invocation scope = per-request header (subagent inheritance works)

Code trail: bot generates `invocation_id` (uuid v4) → `progress_register` on the aggregator's internal Unix socket (`worker.rs:2889-2915`) → bot writes per-invocation `mcp-{id}.json` adding **`PROGRESS_INVOCATION_HEADER: \<invocation_id\>` into `mcpServers.right.headers`** next to the Bearer (`cc/invocation.rs:139-173`) → aggregator reads `context.invocation_id` **from each request's header** and resolves `(chat_id, thread_id)` (`right_backend.rs:1239-1245`); missing header → `conversation_scope_unavailable` (`:1914`).

**Consequence:** any process connecting with the same headers inherits the scope. pi/omp subagents read the same `mcp.json` as the parent → children get scoped tools (`send_message`, `thread_search`, `cron_*`) scoped to the parent's (chat, thread), sharing the 20-calls/turn cap. The "scope never from agent args" invariant holds (id comes from a bot-written file and maps to exactly one conversation). Whether children SHOULD see `send_message` is a policy knob (adapter `excludeTools` / agent frontmatter `tools:`), not a mechanism gap. **The v2 "invocation family" aggregator change is NOT needed.** Verified live: ad-hoc connection without a registered invocation gets `conversation_scope_unavailable`.

### 10.2 The omp "fresh-profile MCP bug" was NULL_PROMPT × xdev (solved, with fix)

A 3-hour bisect (12+ probe runs across profiles, projects, caches, models, races) ruled out: profile isolation, project trust, agent.db settings, tool cache, attach timing/race, server payload, protocol version. The discriminator: **`NULL_PROMPT=true`**. Mechanism:

- With `tools.xdev=true` (default), MCP tools attached at startup are exposed to the model **via the xd:// device inventory rendered into the system prompt**. `NULL_PROMPT=true` zeroes the system prompt (`system-prompt.ts:524-526`) → startup-attached MCP tools become **invisible** (they register: handshake + tools/list complete — verified on the wire — but the model never sees them).
- Mid-turn attaches arrive as inventory **notices** (turn-level injections, not system prompt) → visible even under NULL_PROMPT. This is why slower user-scope servers (context7 etc.) appeared in fresh profiles while fast localhost servers (probe, aggregator) didn't.
- **Fix: `tools.xdev: false`** (config.yml or `--config` overlay) → MCP tools register as **native direct tools** (function-calling payload), prompt-independent. Verified: NULL_PROMPT + xdev:false → direct `mcp__probe_ping` call succeeds; byte-exact prompt preserved (tool defs ride the provider's tool channel, not prompt tokens).

**Port requirement:** RightClaw ships `tools.xdev: false` in the sandbox agent config (or per-invocation `--config` overlay) whenever `NULL_PROMPT` is used.

### 10.3 S3 sandbox E2E — **GREEN (omp)**

`omp-linux-arm64` in `test-sandbox-20260516-1649` (aarch64/glibc 2.39): with the xdev fix, **all 43 aggregator tools registered** (`mcp__right_bootstrap_done`, `mcp__right_browser_use_*`, `mcp__right_chat_search`, `mcp__right_composio_*`, …) and a live `mcp__right_chat_search` call returned the correct `conversation_scope_unavailable` (expected without a registered invocation). Full path: sandbox omp → policy egress → `host.openshell.internal:8100` → Bearer → tools/call → structured error. [E]

### 10.4 Criteria-weighted comparison (user's five)

| Criterion | omp 17 | pi 0.80 + extensions | Edge |
|---|---|---|---|
| **Subagents stable+efficient** | builtin `task`: isolated settings snapshot, own JSONL transcript, worktree isolation, `spawns` allowlist + `taskDepth`, 500KB/5000-line caps, sync/async/batch, AJV `outputSchema`, model roles (`@task`/`@smol`), revivable children, **+ peer IRC (`hub` tool, `irc_message` events, `irc.timeoutMs`)** | pi-subagents (community, 113K dl/mo): builtin roles, chains/parallel/background, watchdog, model overrides, lifecycle artifacts. **No peer IRC** (children report to parent) | **omp** |
| **Token efficiency** | hashline compact patch format (edit tokens + stale-anchor rejection), prewalk (plan strong → execute cheap), model roles (smol/tiny), `tools.format` dialects (fewer weak-model retries), snapcompact; xd:// schema-on-demand (incompatible w/ NULL_PROMPT) | lean default prompt; adapter **proxy mode** (~200 tokens for the whole MCP surface, NULL_PROMPT-compatible); datetime removed for cache stability; subagent context isolation | **omp** (more levers; both mitigate the 43-tool surface) |
| **Programmable access** | `--mode rpc` (full bidirectional JSON-RPC), `rpc-ui`, **`--mode acp` native (Zed ACP)**, Python `omp_rpc` wheel, rich ExtensionAPI (25+ events, registerTool/Command/Flag) | `--mode rpc` (prompt/steer/follow_up/set_model/compact/fork), pi-orchestrator (experimental), compiled SDK (cleaner for consumers), ACP via third-party pi-acp (MVP) | **omp** |
| **Forks** | `--fork` verified (new id, full parent context); hidden flag | `--fork` + `SessionManager.forkFrom` + in-place tree (`branch`/`branchWithSummary`/`createBranchedSession`) + `--session-id` create-if-missing | **parity** (pi richer API) |
| **Auto-skills** | builtin autolearn (`learn`/`manage_skill`, managed-skills controller) | none | **moot** — RightClaw's own learning pipeline replaces it; both pick up bot-written `rightx-*` SKILL.md |

### 10.5 Round-15 verdict

**omp** wins the user-weighted matrix (3/5 + parity on forks + auto-skills moot) and is empirically green on EVERY axis: S1 structured output 18/18, S2 setup-token, S3 sandbox E2E with 43 live tools, forks, `.claude/skills`, byte-exact prompt (`NULL_PROMPT` + `tools.xdev:false` + `--append-system-prompt`), MCP with invocation-header inheritance for subagents, Kimi device-flow OAuth built in (pi lacks it). Costs accepted with eyes open: single-maintainer governance + monthly majors (**mitigation: pin + vendor the binary, deliberate upgrade events — same posture as OpenShell alpha**), 166MB glibc binary, `mcp__right__*`→`mcp__right_*` rename, no `--session-id` (capture uuid from the first-turn `session` event), MCP-health via bot-side aggregator probe (both harnesses degrade silently — redesign needed regardless).

**pi + pi-mcp-adapter (+pi-subagents) remains the strong fallback**: better core governance, 43MB, cleaner `terminate:true` structured output, `--session-id`; priced at: no native IRC/dialects/hashline/prewalk/roles, community-extension compatibility matrix, no Kimi OAuth, its sandbox E2E not yet run.

Remaining pre-port work: **S4** (bot-side MCP-health probe design), **S6** (pin/vendor strategy), **S8** (codex `auth.json` regen flow — only if pi path chosen; omp reads `ANTHROPIC_OAUTH_TOKEN`/env for all and has native Kimi OAuth).
