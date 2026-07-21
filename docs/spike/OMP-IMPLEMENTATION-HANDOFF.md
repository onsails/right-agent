# OMP Implementation Handoff — replacing `claude -p` with omp in right-agent

> **Status:** research DONE, implementation NOT started. This doc is the kickoff
> briefing for the implementation session. Capstone research: `docs/spike/OMP-PI-DEEPDIVE.md`
> §10 (round 15), same branch. Branch: `worktree-harness-migration-research` @ `6a1e1c3d`.
> Date: 2026-07-22.

## Mission

Replace RightClaw's per-turn harness invocation
(`ssh openshell-<sandbox> 'claude -p --output-format stream-json --resume <id> …'`)
with **oh-my-pi (omp)** as the sandbox-side agent engine inside OpenShell sandboxes.
User decision (2026-07-18): **omp over pi**. Kimi OAuth deferred
("пока не делаем, у пи появится кими код позже сам" — note omp already HAS Kimi
device-flow OAuth; pi doesn't). Multi-provider auth = **own universal host-side
OAuth flow for ALL providers**, not just OpenAI.

## Locked decisions (do not re-litigate)

| Decision | Choice | Why |
|---|---|---|
| Engine | **omp** | User criteria: subagents+peer IRC, token efficiency (hashline/prewalk/model roles/dialects), programmable access (native ACP + rpc + Python wheel). Fork parity with pi. Auto-skills moot (RightClaw has own learning pipeline). |
| Structured output | **Mechanism-C port**: extension registers `structured_output` tool, plain JSON Schema as `parameters`; loop-level validation retries | Verified 18/18 schema-valid across claude-sonnet-5, kimi-code/k3, gpt-5.4-mini incl. cron oneOf + prefilter schemas. omp has no generic `terminate` — trailing text after accepted call is harmless (bot ignores). |
| Session IDs | **Capture uuidv7 from first-turn `session` event** | omp has NO `--session-id` flag. Resume/fork work off captured id. `--fork` is hidden flag, needs persistence. |
| Prompt delivery | **`NULL_PROMPT=true` + `tools.xdev:false` + `--append-system-prompt`** | Byte-exact composite prompt (89 input tokens vs 31,886 default) AND MCP tools visible. `xdev:true` + `NULL_PROMPT` = invisible xd:// tool inventory → the 3-hour "fresh-profile MCP bug". |
| Subagent MCP scope | **No aggregator v2 change** — per-invocation header inheritance | Subagents share parent's bot-written `mcp-{id}.json` → inherit `PROGRESS_INVOCATION_HEADER` scope. Policy knob (`excludeTools` / frontmatter `tools:`) if children must not see `send_message`. |
| Credentials | **`provider_credentials` table in per-agent data.db** (provider, kind, access_token, refresh_token, expires_at); bot refreshes host-side before injection | Anthropic = setup-token paste (`ANTHROPIC_OAUTH_TOKEN` env); OpenAI = own OAuth flow; Kimi = deferred. |
| Distribution | **Pin + vendor omp binary from RightClaw mirror**, deliberate upgrade cadence | Same posture as OpenShell alpha. omp costs accepted: single maintainer, monthly majors, 142–166MB glibc binary. |
| Rollout | **`harness: claude\|omp` in agent.yaml**, new agents default omp | Upgrade-friendly: existing agents keep claude until flipped. |

## Verified omp facts (empirical, local 17.0.0 nix + clone @ `3fdd85ab` = v17.0.4; npm latest 17.0.4)

- **Binary**: single bun-compiled binary; linux-arm64 142MB, linux-x64 166MB, glibc≥2.17, **no musl**. No daemon needed for `-p`.
- **Sandbox deployment PROVEN** (S3 GREEN): `cat omp-linux-arm64 | ssh … "cat > /sandbox/.local/bin/omp && chmod +x"` + sha256 both sides. Runs with `HOME=/sandbox PI_CODING_AGENT_DIR=/sandbox/.omp-test PI_SKIP_VERSION_CHECK=1` in `test-sandbox-20260516-1649` (aarch64, glibc 2.39).
- **Print mode**: `omp -p --mode json` → NDJSON events: `session`, `agent_start`, `turn_start`, `message_*`, `tool_execution_*`, `turn_end`, `agent_end`.
- **Sessions**: `~/.omp/agent/sessions/<mangled-cwd>/<ts>_<uuidv7>.jsonl`; subagent children under `<uuid>/`. `PI_CODING_AGENT_DIR` redirects agent dir. `--resume` verified (99% cacheRead on 2nd turn). `--fork` = new id, full parent context.
- **MCP config discovery**: `~/.omp/agent/mcp.json` (user-level, HOME-resolved — **NOT** redirected by `PI_CODING_AGENT_DIR` for capability loading) + `<cwd>/.omp/mcp.json` + `<cwd>/.mcp.json` (claude-compatible). Bearer sent on every request. `OMP_MCP_TIMEOUT_MS` (default 30s).
- **MCP naming**: `mcp__<server>_<tool>` — **single underscore**, NOT claude's `mcp__right__*`. Server-key trick defeated by sanitizer. Rename sweep required (see Work items).
- **Dead MCP server = SILENT**: turn succeeds, exit 0, no event signal → S4 health probe needed bot-side.
- **stdin merged with argv prompt** → pin stdin: `</dev/null`.
- **43 aggregator tools registered live in sandbox**; `mcp__right_chat_search` → correct `conversation_scope_unavailable`.
- **Config**: 438 keys; `--config <overlay.yml>` repeatable. Notable: `tools.format` dialects (auto\|native\|glm\|hermes\|kimi\|xml\|anthropic\|deepseek\|harmony\|qwen3\|gemini\|gemma\|minimax), `mnemopi.*` off by default, `hindsight.*` section exists (possible native Hindsight integration — RightClaw's memory backend; investigate, low priority), `autolearn.*` must be disabled, `tools.xdev:false` (mandatory, see prompt combo).
- **Auth env**: `ANTHROPIC_OAUTH_TOKEN` / `ANTHROPIC_API_KEY` — **NOT** `CLAUDE_CODE_OAUTH_TOKEN`.
- **Providers**: 36 native incl. `kimi.ts` + Kimi device-flow OAuth.
- **Missing flags vs claude**: no `--json-schema`, `--max-turns`, `--max-budget-usd`, `--mcp-config`, `--session-id`. Replacements: extension tool (schema), config/agency (turns), host-side budget gate (RightClaw already has one), per-invocation config file written to omp's discovery path, session-event capture.
- **Skills**: `.claude/skills/*/SKILL.md` project drop-in works (ClawHub-compatible).
- **Task tool**: frontmatter `{name,description,tools,spawns,model,thinkingLevel,blocking,prewalk}`; `outputSchema` AJV; `taskDepth`; 500KB/5000-line caps; peer IRC (`hub` tool, `irc_message` events); `yield` tool validates vs `session.outputSchema` (subagent path).
- **Extension API**: `export default function(pi){ pi.registerTool({name,label,description,parameters,loadMode:"essential",execute}) }`, loaded via `-e file.ts`.
- **"Efficient Rust core" reality-check**: `pi-natives`/`pi-ast`/`pi-shell`/`pi-walker`/`pi-uu-*` accelerate tool *execution* (grep/PTY/ast/diff in-process), NOT the tool-call loop; marginal for RightClaw (critical path = LLM latency + MCP round-trips). `hashline` (TS) is the real tool-call token optimization.

## S1/S2/S3 results

- **S1 (structured output, mechanism-C): PASS.** omp 18/18 schema-valid (one self-corrected retry); pi 2/2 cleaner (`terminate:true`). Extension artifact: `/tmp/omp-probe/struct-ext.ts` (structural types, no `: any`). Runs archived: `/tmp/s1-runs/*.ndjson`.
- **S2 (setup-token auth): PASS.** Agent `agent-b` `sk-ant-oat01-` token as `ANTHROPIC_OAUTH_TOKEN`, isolated `PI_CODING_AGENT_DIR`; claude-sonnet-5 turn completed, nothing persisted. Direct-curl control: 429 (rate-limited but authenticated).
- **S3 (sandbox E2E): PASS.** Binary upload sha256-verified; full aggregator tool surface live; scope resolution correct.

## RightClaw contract points (code to touch)

| File | What |
|---|---|
| `crates/bot/src/cc/invocation.rs:502-617` | `ClaudeInvocation::into_args` → replace with `OmpInvocation` builder; invariants enforced at compile time |
| `crates/bot/src/cc/invocation.rs:139-173` | `with_progress_invocation_header` + `write_invocation_mcp_config` → per-invocation `mcp-{id}.json` write to omp discovery path |
| `crates/bot/src/cc/invocation.rs` | `guard_no_sandboxed_host_exec` — keep, applies to omp identically |
| `crates/bot/src/cc/stream.rs` | Full rewrite: omp event taxonomy (`session`→init+id capture, `turn_end`/`agent_end`→result+usage, `message_*`/`tool_execution_*`→assistant/user blocks, `structured_output` `isError`→schema-rejection detector, abort-after-3→reflection preserved) |
| `crates/bot/src/cc/prompt.rs:289-291` | Composite prompt `--system-prompt-file` → omp `--append-system-prompt` (or argv/stdin) under `NULL_PROMPT=true` |
| `crates/bot/src/telegram/worker.rs:2889+` | `start_progress_invocation` — header mechanism unchanged |
| `crates/right/src/right_backend.rs:1239-1245` | Aggregator scope resolution from header — unchanged |
| `crates/right/src/right_backend.rs:1914` | `conversation_scope_unavailable` — unchanged |
| `crates/bot/src/idle_compaction.rs` | Specialized callsite: `/compact` has no omp equivalent → decide: drop compaction, or emulate via fork+truncate; needs design decision early |
| `crates/bot/src/reflection.rs`, `crates/bot/src/cron.rs` | Reflection/cron invocations → route through new builder |

## Implementation work plan (slices, each with own verification)

1. **W1 `OmpInvocation` builder** — flag map: `-p --mode json`, `--resume/--fork`, `--model`, `--no-session`, `-e struct-ext`, `--config` overlay (`tools.xdev:false`, `autolearn` off), `NULL_PROMPT=true`, `--append-system-prompt`, stdin pinned `</dev/null`. Compile-time invariant enforcement mirroring `ClaudeInvocation`. → verify: unit tests on arg vectors + guard.
2. **W2 stream parser** (`stream.rs` rewrite) + session-id capture from first-turn `session` event. → verify: replay archived `/tmp/s1-runs/*.ndjson` through parser; golden-event fixtures.
3. **W3 structured-output extension bundle** — port `/tmp/omp-probe/struct-ext.ts`; bot-side `isError` rejection detector; abort-after-3→reflection. → verify: schema-valid rate vs cron oneOf + prefilter schemas, target = S1 baseline (18/18).
4. **W4 per-invocation MCP config** — write `mcp-{id}.json` to `<cwd>/.mcp.json`-equivalent discovery path with `Authorization` + `PROGRESS_INVOCATION_HEADER`; subagent inheritance via shared file. → verify: live sandbox, scope-correct `chat_search`.
5. **W5 sandbox codegen** — binary + extension bundle + `config.yml` deployment as `Regenerated(BotRestart)` via `contract.rs` writers; registry entry; pin/vendor mirror fetch. → verify: `registry_covers_all_per_agent_writes`, fresh-agent bootstrap end-to-end.
6. **W6 multi-provider credentials** — `provider_credentials` migration in `right_db::migrations::MIGRATIONS`; host-side refresh before injection; universal OAuth flow (all providers); `/providers` dashboard onboarding. → verify: token refresh round-trip; setup-token path regression (S2 harness).
7. **W7 subagents** — bot-orchestrated flows port first; agent-facing `task` tool policy (scope inheritance works; `excludeTools` knob for `send_message`). → verify: fork-heavy background flow parity vs claude.
8. **W8 S4 MCP-health probe** — aggregator tracks per-token last-seen; bot checks before delivering (omp degrades silently on dead MCP). → verify: killed-upstream scenario surfaces to user instead of silent success.
9. **W9 rename sweep** `mcp__right__*` → `mcp__right_*` across `skills/`, `templates/right/`, `crates/right-agent/src/codegen/agent_def.rs`, `PROMPT_SYSTEM.md`, `with_instructions()` in `memory_server.rs` + `aggregator.rs`. → verify: grep-clean + prompt-assembly tests.
10. **W10 rollout** — `harness: claude|omp` in `agent.yaml` (default claude for existing, omp for new); learning pipeline callsites (Haiku prefilter, probe-writer fork, curator) ported; idle-compaction decision executed. → verify: full workspace nextest + doctests (mandatory final gate per AGENTS.md).

Sequencing: W1→W2→W3→W4 are the critical path (one agent turn end-to-end). W5/W6/W9 parallelizable. W7/W8 after W4. W10 last.

## Environment cheatsheet

- Sandbox SSH: `ssh -F ~/.right/run/ssh/<sandbox>.ssh-config openshell-<sandbox-name>`; sandboxes: `test-sandbox-20260516-1649` (Ready, aarch64), `test-sandbox-20260516-1640`, `test-sandbox-a`.
- Agent token: `sqlite3 -readonly ~/.right/agents/<name>/data.db "SELECT token FROM auth_tokens ORDER BY created_at DESC LIMIT 1;"` — **never print**; shell var only.
- Sandbox mcp.json: `/sandbox/mcp.json` (server "right", `http://host.openshell.internal:8100/mcp`, Bearer). Aggregator reachable; 401 without auth.
- omp run env in sandbox: `HOME=/sandbox PI_CODING_AGENT_DIR=<isolated> PI_SKIP_VERSION_CHECK=1`.
- Worktree rule: never bare `right`; use `cargo run --bin right -- …` or `target/devenv/<profile>/right`.
- Commands: `devenv shell -- cargo nextest run -p <crate> <filter>`; final gate: `devenv shell -- cargo nextest run --workspace` + `devenv shell -- cargo test --doc --workspace`.

## Risks / gotchas

- **Silent MCP degradation** (omp exit 0 on dead server) → W8 is not optional for production parity.
- **No `--max-turns`/`--max-budget-usd`** → cron budget enforcement must live in bot (parse usage from `turn_end`/`agent_end` events; kill subprocess on cap).
- **Idle compaction**: `/compact` doesn't exist in omp. Decide early: drop, or emulate (fork + summarize + new session). Affects `SessionLocks` assumptions.
- **Monthly omp majors** → pin exact version in mirror; upgrade = deliberate PR with S1/S3 regression rerun.
- **`~/.omp/agent/mcp.json` is HOME-resolved, not `PI_CODING_AGENT_DIR`-redirected** for capability loading — tests must set HOME explicitly or use project-level discovery paths.
- **Learning pipeline** has 3 specialized callsites (prefilter, probe-writer fork, curator) with their own contracts — see `docs/architecture/learning.md`; each needs an omp port + `LEARNING_SOURCES` accounting unchanged.
- **`pi-natives` acceleration is NOT a loop optimization** — don't expect latency wins; sell the migration on subagents/token-efficiency/programmability.
- **FAIL-FAST** per AGENTS.md: every error propagates; never print secrets; Conventional Commits; TDD narrowest-loop; final full-workspace gate mandatory.

## Artifacts

- This doc + capstone: branch `worktree-harness-migration-research`, `docs/spike/OMP-IMPLEMENTATION-HANDOFF.md`, `docs/spike/OMP-PI-DEEPDIVE.md` (§10).
- Worktree on disk: `.claude/worktrees/harness-migration-research/`.
- Ephemeral probe artifacts in /tmp (recreate if gone): `/tmp/omp-probe/struct-ext.ts` (also archived in repo at `docs/spike/harness/omp_mcp_probe.py` — the dummy MCP server), `/tmp/omp-probe/mcp_proxy.py`, `/tmp/s1-runs/*.ndjson`, `/tmp/pi-test/`, `/tmp/omp-linux-arm64` (upload-tested binary), `/tmp/omp-src` (clone @ `3fdd85ab`).
- pi fallback (if omp regresses): `@earendil-works/pi-coding-agent@0.80.10` + pi-mcp-adapter (132.6K dl/mo) + pi-subagents; has `--session-id`, `terminate:true`, 43MB; NO Kimi OAuth; openai-codex OAuth reads auth.json only.
