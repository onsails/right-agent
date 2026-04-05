---
phase: v3.2-mcp-oauth-uat
verified: 2026-04-05T00:00:00Z
status: human_needed
score: 5/6 must-haves verified
human_verification:
  - test: "Run rightclaw up with a configured cloudflared tunnel, then inspect the generated ~/.rightclaw/scripts/cloudflared-start.sh"
    expected: "exec line reads: exec cloudflared tunnel run --config /home/.../.rightclaw/cloudflared-config.yml --token ..."
    why_human: "Script is generated at runtime by cmd_up — cannot verify the output file without actually running rightclaw up with a live tunnel config"
  - test: "Start rightclaw bot with an agent that has an unauthenticated MCP server in .mcp.json"
    expected: "Bot startup logs contain WARN lines: 'MCP server needs auth' with server=<name> and state=auth required"
    why_human: "Bot startup is a runtime event; tracing output requires an actual bot process"
  - test: "Send /mcp list in Telegram to the bot"
    expected: "Response shows lines formatted as '  ✅ notion  —  present' (emoji icon, server name, em-dash, AuthState Display label)"
    why_human: "Telegram message rendering is live bot behavior"
  - test: "Send /doctor in Telegram to the bot"
    expected: "Response renders doctor check output inside a monospace code block (HTML <pre> tag via ParseMode::Html)"
    why_human: "Telegram ParseMode rendering is live bot behavior"
---

# Phase v3.2 UAT Gap Fixes: Verification Report

**Phase Goal:** Fix all five diagnosed UAT gaps so every test in the v3.2 UAT passes.
**Verified:** 2026-04-05
**Status:** human_needed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `rightclaw mcp status` prints per-agent HTTP MCP auth table | ✓ VERIFIED | `McpCommands::Status`, dispatch arm, and `cmd_mcp_status` function all present in main.rs lines 81-242, 1099-1145 |
| 2 | `rightclaw mcp status --agent <name>` filters to that agent | ✓ VERIFIED | `Status { agent: Option<String> }` with `--agent` long flag; `cmd_mcp_status` path-checks agent dir and returns "agent not found" error |
| 3 | cloudflared-start.sh exec line contains `--config` flag | ✓ VERIFIED (code) / ? HUMAN (runtime file) | main.rs line 589: `exec cloudflared tunnel run --config {cf_config_path_str} --token {token}` with `cf_config_path_str = cf_config_path.display()` |
| 4 | Bot startup logs `tracing::warn!` per unauthenticated MCP server | ✓ VERIFIED (code) / ? HUMAN (runtime) | bot/src/lib.rs lines 108-129: `mcp_auth_status` check after `getMe`, emits `tracing::warn!` with agent/server/state fields |
| 5 | Telegram /mcp list shows ✅/❌/⚠️ icons with proper labels | ✓ VERIFIED (code) / ? HUMAN (Telegram) | handler.rs lines 261-266: emoji match + `format!("  {} {}  —  {}\n", icon, s.name, s.state)` |
| 6 | Telegram /doctor output wrapped in code block | ✓ VERIFIED (code) / ? HUMAN (Telegram) | handler.rs lines 634-638: `format!("Doctor results:\n\n<pre>{}</pre>", body)` with `ParseMode::Html` |

**Score:** 5/6 truths fully verified in code (SC #6 cargo build/test confirmed by orchestrator). All 4 Telegram-runtime behaviors require human testing.

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/rightclaw-cli/src/main.rs` | McpCommands enum, Commands::Mcp variant, cmd_mcp_status fn, --config flag in cloudflared script | ✓ VERIFIED | All four pieces present; tests for variant existence, agent-not-found error, no-.mcp.json success |
| `crates/bot/src/lib.rs` | mcp_auth_issues check in run_async startup | ✓ VERIFIED | Lines 108-129: exact code from PLAN inserted after getMe block, before cron spawn |
| `crates/bot/src/telegram/handler.rs` | emoji icons in handle_mcp_list, HTML pre block in handle_doctor | ✓ VERIFIED | Lines 261-266 (emoji + AuthState Display), lines 634-638 (pre + ParseMode::Html) |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `Commands::Mcp` dispatch arm | `cmd_mcp_status` | `match cli.command` at line 240 | ✓ WIRED | `Commands::Mcp { command } => match command { McpCommands::Status { agent } => cmd_mcp_status(&home, agent.as_deref()) }` |
| cloudflared-start.sh exec line | cloudflared-config.yml ingress rules | `--config {cf_config_path_str}` flag | ✓ WIRED | `cf_config_path_str = cf_config_path.display()` injected at line 587; format string at line 589 |
| `bot run_async` | `rightclaw::mcp::detect::mcp_auth_status` | `tracing::warn!` after deleteWebhook | ✓ WIRED | Called at line 109 with agent_dir and credentials_path; warn emitted per non-Present server |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|--------------|--------|-------------------|--------|
| `handle_mcp_list` | `statuses` | `rightclaw::mcp::detect::mcp_auth_status(&mcp_path, &cred_path)` | Yes — reads .mcp.json + .credentials.json | ✓ FLOWING |
| `handle_doctor` | `checks` | `rightclaw::doctor::run_doctor(&home.0)` | Yes — live doctor checks | ✓ FLOWING |
| `cmd_mcp_status` | `statuses` | `mcp_auth_status(&mcp_path, &credentials_path)` per agent dir | Yes — reads .mcp.json + .credentials.json | ✓ FLOWING |

### Behavioral Spot-Checks

Step 7b: SKIPPED for Telegram bot behaviors (live bot required). CLI subcommand wiring verified statically via grep and test inspection.

| Behavior | Evidence | Status |
|----------|----------|--------|
| `rightclaw mcp` shows clap help | `McpCommands` has `#[derive(Subcommand)]` — clap auto-generates help | ✓ PASS (static) |
| `--agent nonexistent` returns non-zero | Test `cmd_mcp_status_errors_on_nonexistent_agent` verified at line 1519 | ✓ PASS (test) |
| no-.mcp.json path succeeds | Test `cmd_mcp_status_returns_ok_with_no_mcp_json` at line 1536 | ✓ PASS (test) |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|---------|
| DETECT-01 | v3.2-gaps-PLAN | mcp status CLI table | ✓ SATISFIED | `cmd_mcp_status` function + dispatch arm |
| DETECT-02 | v3.2-gaps-PLAN | `--agent <name>` filter | ✓ SATISFIED | `Status { agent: Option<String> }` with path check |
| OAUTH-02 | v3.2-gaps-PLAN | cloudflared config flag | ✓ SATISFIED | `--config {cf_config_path_str}` in script template |
| BOT-02 | v3.2-gaps-PLAN | /mcp list emoji icons | ✓ SATISFIED | `✅/❌/⚠️` match + AuthState Display format |
| BOT-03 | v3.2-gaps-PLAN | /doctor code block | ✓ SATISFIED | `<pre>...</pre>` with `ParseMode::Html` |

### Anti-Patterns Found

None. The implementation matches the plan's prescribed patterns exactly. No TODOs, no placeholder returns, no hardcoded empty data. All error paths propagate via `?` or `tracing::warn!` as appropriate.

### Human Verification Required

#### 1. cloudflared-start.sh runtime content

**Test:** Run `rightclaw up` with a configured cloudflared tunnel token, then read `~/.rightclaw/scripts/cloudflared-start.sh`
**Expected:** The exec line reads `exec cloudflared tunnel run --config /home/.../.rightclaw/cloudflared-config.yml --token <token>`
**Why human:** Script is written to disk by `cmd_up` at runtime — the file does not exist without a live `rightclaw up` run

#### 2. Bot startup MCP auth warnings

**Test:** Start `rightclaw bot --agent <agent>` where `<agent>` has an unauthenticated HTTP MCP server in `.mcp.json`
**Expected:** Logs between the `deleteWebhook` confirmation and cron spawn contain `WARN MCP server needs auth server=<name> state=auth required`
**Why human:** `tracing::warn!` output requires a live bot process with tracing subscriber active

#### 3. /mcp list Telegram output

**Test:** Send `/mcp list` in Telegram to the agent bot
**Expected:** Response shows `✅ notion  —  present` style lines with emoji icon, server name, em-dash, and AuthState Display label
**Why human:** Telegram message rendering is a live network interaction

#### 4. /doctor Telegram output

**Test:** Send `/doctor` in Telegram to the agent bot
**Expected:** Doctor output appears in a monospace code block (not plain text) — visually distinct from surrounding text
**Why human:** `ParseMode::Html` rendering with `<pre>` tags is a live Telegram client behavior

### Gaps Summary

No gaps — all code changes are present, substantive, and wired. The 4 human verification items are runtime/Telegram behaviors that cannot be verified statically. The orchestrator confirmed cargo build --workspace passes (0 errors) and cargo test --workspace passes (466 passed, 1 pre-existing failure unrelated to this phase).

### Commits

All three SUMMARY-documented commits verified in git log:
- `b8d0d3b` — feat(v3.2-gaps): restore rightclaw mcp status CLI subcommand
- `606225f` — fix(v3.2-gaps): cloudflared --config flag + bot startup MCP warn
- `3d9ed1c` — fix(v3.2-gaps): /mcp list emoji icons + /doctor HTML code block

---

_Verified: 2026-04-05_
_Verifier: Claude (gsd-verifier)_
