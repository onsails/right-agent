@AGENTS.rust.md

This is a Rust project. Follow conventions in AGENTS.rust.md.

## Project

**Right Agent**

Right Agent is an opinionated, closed-box AI agent platform — peer to OpenClaw and Hermes in category. Every choice is made for you, security is the default, and we polish what ships before adding more. Built on Claude Code running inside NVIDIA OpenShell sandboxes, orchestrated by process-compose. Drop-in compatible with the OpenClaw/ClawHub ecosystem at the file level (same conventions, same skill format, same registry) — but with security-first enforcement instead of "grant all, pray it works."

**Core Value:** One Telegram bot per agent — every chat its own Claude Code session over a shared, chat-tagged memory. Every agent in its own sandbox, every credential outside it. The box is closed; you just use it.

### Constraints

- **Language**: Rust (edition 2024)
- **Dependencies**: process-compose (external), microsandbox SDK (crate), Claude Code CLI (external)
- **Platforms**: Linux and macOS
- **Compatibility**: Drop-in compatible with OpenClaw file conventions and ClawHub SKILL.md format
- **Security**: Every agent runs in an Agent Sandbox — a microVM managed through the microsandbox SDK (`right-sandbox`). There is no sandboxless mode. Always `--dangerously-skip-permissions`; the microVM boundary and its egress policy are the security layer.
- **Sandbox runtime**: the SDK pins its own `msb` runtime and installs on first use (`right_sandbox::ensure_runtime_installed`). macOS needs Apple Silicon; Linux needs KVM.
- **Stack**: `Cargo.toml` is the source of truth for dependencies. Project standards in `AGENTS.rust.md`.

## Docs

- Always commit `docs/superpowers/` spec and plan files. Never leave them untracked.

## Verification cadence

- Tests are milestone checks, not punctuation after every edit. Batch small edits and avoid repeating cargo checks/tests when the signal would be the same.
- At the start of substantial feature work or a new worktree, run one baseline verification appropriate to the scope. Prefer targeted package/module tests for narrow work; use full workspace tests only when the change is broad or the baseline is uncertain. Record any pre-existing failures.
- During implementation, prefer the narrowest useful command (`devenv shell -- cargo nextest run -p <crate> <filter>`, package-level tests, or a targeted build/check) after a TDD red/green loop or a coherent feature slice. `cargo nextest run` is the recommended runner; doctests run only under `cargo test --doc`.
- At the end of all code work, including work done inside a worktree, `devenv shell -- cargo nextest run --workspace` plus `devenv shell -- cargo test --doc --workspace` is mandatory. Targeted tests do not replace the final full workspace test.
- New `docs/superpowers/` plans must encode this cadence: targeted intermediate verification and one final full workspace test, not full workspace tests after every task.
- **Website-only work skips Rust tests.** When a change touches only the website (`site/`) and no Rust crate, do not run `cargo` tests, checks, or builds — they verify nothing relevant. Verify with the website's own tooling (build/lint/preview under `site/`) instead. The Rust full-workspace mandate above applies only when Rust code changed.

## Conventions

- **Bot-first management**: MCP management goes through the Telegram Mini App dashboard opened by `/mcp`; model/runtime controls such as `/model` remain bot-managed. Never create or edit `.mcp.json`, agent configs, or credential files manually — the bot/dashboard is the control plane.
- **Provider management**: Provider management goes through the Telegram Mini App dashboard opened by `/providers`. Never create or edit gateway providers via host CLI or `agent.yaml` directly — the bot/dashboard is the control plane.
- **Debuggability over convenience**: Always prefer direct, observable signals over indirect heuristics. If an API provides status, use it — don't infer status from side effects (e.g. SSH connectivity as a proxy for sandbox readiness). Errors must propagate to logs, never be silently swallowed.
- **Domain research before implementation**: Always verify external tool APIs by reading source code or running `--help` before writing integration code. Never rely solely on web documentation — it may be outdated or wrong.
- **PROMPT_SYSTEM.md**: Always keep PROMPT_SYSTEM.md in sync with the actual prompting system. When changing system prompt generation, agent definitions, JSON schemas, or MCP instructions, update PROMPT_SYSTEM.md to match.
- **Prompt-tier brevity**: Files that ship to every CC invocation as part of the composite system prompt — `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, `CRON_INSTRUCTIONS.md`, `templates/right/agent/BOOTSTRAP.md`, and the base prompt assembled in `crates/right-codegen/src/agent_def.rs::generate_system_prompt` — are paid for in tokens on every turn. State rules in 1-3 sentences, prefer declarative facts over imperative narration, cut JSON/YAML examples when prose already covers the rule, and avoid duplicating anything already in another section of the same composite prompt. Long narration belongs in `PROMPT_SYSTEM.md` (operator-facing), not in the prompt itself. The base prompt (`generate_system_prompt`) and `OPERATING_INSTRUCTIONS.md` must not duplicate rules; their split follows the boundary invariant in `PROMPT_SYSTEM.md` (base = parameterized values + the Bootstrap-universal minimum; OPERATING = operating-only procedure).
- **MCP with_instructions()**: When adding, removing, or renaming MCP tools, always update `with_instructions()` in both `memory_server.rs` and `aggregator.rs` to reflect the current tool set and descriptions.
- **MCP tool names in agent-facing text**: CC prefixes MCP tools as `mcp__{server}__{tool}`. The Right Agent server is `"right"`, so agents see `mcp__right__<tool>`. All skills, templates, prompts, and codegen that reference tool names for agents must use the full prefixed form. When adding, removing, or renaming tools, update references in: `skills/`, `templates/right/`, `crates/right-agent/src/codegen/agent_def.rs`, `PROMPT_SYSTEM.md`.
- **Debugging agent sessions**: In development, bots run with `--debug`. Three log sources: (1) CC debug logs inside the sandbox at `/sandbox/.claude/logs/`; (2) stream NDJSON logs at `~/.right/logs/streams/<session-uuid>.ndjson` on host; (3) process-compose per-process logs via REST API: `curl -s "http://localhost:18927/process/logs/{process-name}/0/50"` (e.g. `right-mcp-server`, `right-bot`). Bot and aggregator are separate processes — always check both when debugging MCP issues.
- **Reproduce a sandbox `claude` invocation by hand**: There is no SSH into an Agent Sandbox — the microsandbox SDK is the only transport, so drive it from a small Rust binary or test that attaches with `right_sandbox::SandboxHandle::attach(&name)` and runs an `ExecRequest` for `claude`, exactly as the bot does in `build_claude_command` (`crates/bot/src/cc/invocation.rs`). Resolve the name with `right_sandbox::resolve_sandbox_name(agent, explicit)`. The agent's Claude OAuth token lives in its per-agent DB (`~/.right/agents/<agent>/data.db`, table `auth_tokens`) and must be injected as `CLAUDE_CODE_OAUTH_TOKEN`; read it into a variable and never echo, log, or print it (project rule: never get secrets into context).
- **Worktree binary for `right`**: When operating inside this repo, never invoke bare `right` — `$PATH` may resolve to a stale installed copy. Use `cargo run [--release] --bin right -- <args>` or the explicit `target/devenv/<release|debug>/right`. `right up` bakes `current_exe()` into `process-compose.yaml` and refuses to regenerate while PC is healthy, so one wrong invocation pins the wrong binary until `right down`.
- **Self-healing platform**: Never manually fix agent sandboxes, configs, or state. If a platform change breaks an agent, the platform code must detect and recover automatically (re-upload if files are missing, adjust policy, etc.). Manual fixes mask bugs and prevent proper testing.
- **Never delete sandboxes for recovery**: Sandboxes contain agent data (credentials, installed tools, agent-created files). Deleting a sandbox destroys this data. Platform changes must be designed to work with existing sandboxes — never require sandbox recreation as a migration path.
- **Upgrade-friendly design**: Every new feature must be adoptable by already-deployed agents without recreation. New config fields default to the previous behavior (backward-compatible defaults). `agent config` must expose all user-facing settings — if a feature exists but can't be toggled via CLI, it's incomplete. Think in terms of upgrades, not fresh installs.
  - **Bot-managed fields are a documented exception to the CLI-exposure rule.** Operational/runtime concerns reached over Telegram or the Telegram Mini App dashboard (`/mcp`, `/model`) are intentionally **not** mirrored as `right agent config` flags — the bot/dashboard is the control plane for these, and `agent.yaml` (the source of truth) remains directly user-editable for out-of-band changes.
- **Simplest for the user, most maintainable for us.** When a feature has
  multiple working implementations, choose the one that (a) gives the user
  fewer steps and an explicit, auditable choice, and (b) reuses existing,
  tested paths instead of new control planes or invariant hybrids. Add new
  gateway/sandbox surface only when it is isolated and additive, not when the
  alternative smears complexity across load-bearing machinery.
## Architecture docs split

`ARCHITECTURE.md` is **prescriptive only** — load-bearing rules,
contracts, gotchas, reference tables, schema/protocol invariants. It is
`@`-imported and loads on every conversation; every line costs tokens.

**Hard budget:** ARCHITECTURE.md MUST stay under 40k characters. If an
edit would push it over, cut something else or move content to a
satellite in the same commit. This is non-negotiable — at 40k+ the
CLI warns and downstream conversations pay the cost on every turn.

Descriptive content (data flows, feature walkthroughs, mechanism
narration, "first X happens, then Y, then Z" sequences, helper-method
inventories) lives in `docs/architecture/*.md`. Reference satellites by
**plain path** in `ARCHITECTURE.md` or here — never `@`-import them.
That is the whole point of the split.

**Default for new content is the satellite.** Before adding to
`ARCHITECTURE.md`, the change must clear all three tests:

1. **Rule test:** does it say what code MUST or MUST NOT do? ("X is
   forbidden", "every Y goes through Z", "MUST use helper W") If it
   narrates what happens, it's descriptive — satellite.
2. **Enforcement test:** is there a compiler check, test, or
   review-blocking pattern that catches violations? If it's just "good
   to know", it's descriptive — satellite.
3. **Brevity test:** can the rule be stated in ≤3 sentences (or one
   table row)? If you need a walkthrough to convey it, the walkthrough
   belongs in the satellite and only the rule statement stays here.

**Sentinel phrases that mean "move it out":** "The X subsystem works
by…", "First X, then Y, then Z", "X is implemented as…", "The flow is…",
"This was historically…", numbered step-by-step procedures longer than
3 steps, helper-method bullet lists.

**Cite-on-touch (mandatory):** when modifying a subsystem, re-read the
corresponding `docs/architecture/<x>.md` and update it if drifted. These
docs are not auto-loaded, so they will rot silently if not maintained.
Code is authoritative; the satellite doc is a courtesy to readers.

**When in doubt, put it in the satellite and link from ARCHITECTURE.md
with a one-line summary.** It is always easier to promote a rule later
than to evict descriptive text once it's wedged into the prescriptive
doc.

## Architecture

@ARCHITECTURE.md

Update ARCHITECTURE.md only when **contracts, invariants, or
review-blocking rules** change (new mandatory crate boundary, new
codegen category, new MCP routing rule, new sandbox-policy invariant).
Routine evolution — added features, new data flows, refactored helpers,
new walkthroughs — goes into `docs/architecture/*.md` instead.

## mimo / sprint model selection

When a GPT (OpenAI) model is selected for mimo — including the `sprint` and
`mimo-code` executors — always pin `venice/openai-gpt-55`. The direct `openai/*`
ids (codex and gpt) fail under our ChatGPT-account auth ("not supported when
using Codex with a ChatGPT account"); `venice/openai-gpt-55` is the latest
non-pro OpenAI GPT and works via Venice auth.

## Worktree site dev server — symlink `.env.local`

When running the `site/` dev server from a git **worktree**, always symlink the
main checkout's gitignored `site/.env.local` into the worktree first:
`ln -s <main>/site/.env.local <worktree>/site/.env.local`. `git worktree` does
not carry untracked/gitignored files, and `site/astro.config.mjs` reads
`RIGHT_SITE_DEV_ALLOWED_HOSTS` (dev `vite.server.allowedHosts`, e.g. the
tailnet host) from `.env.local`. Without the symlink the worktree dev server
omits `allowedHosts` and Vite blocks remote/tailnet hosts. Restart the dev
server after linking (astro reads `.env.local` only at startup).

## Agent skills

### Issue tracker

Issues live in GitHub Issues on `onsails/right-agent`, managed via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
