# Milestones

## v3.2 MCP & Tunnel (Shipped: 2026-04-05)

**Phases completed:** 2 phases, 3 plans, 6 tasks

**Key accomplishments:**

- cloudflared spawned as persistent process-compose entry via conditional Jinja2 block, wired from cmd_up with pre-flight PATH check
- Replaced proprietary .credentials.json key-derivation with direct .mcp.json Authorization header injection and _rightclaw_oauth metadata storage
- Completed OAuth token migration from .credentials.json to .mcp.json headers — removed credentials_path from all structs/functions, eliminated PcClient agent restart from callback

---

## v3.1 Sandbox Fix & Verification (Shipped: 2026-04-03)

**Phases completed:** 3 phases, 3 plans, 4 tasks

**Key accomplishments:**

- Fix CC sandbox silent disable in nix by injecting system rg path, failIfUnavailable flag, correcting USE_BUILTIN_RIPGREP polarity, and adding ripgrep to devenv
- DOC-01: `check_rg_in_path() -> DoctorCheck`
- 4-stage bash pipeline in tests/e2e/verify-sandbox.sh confirms CC sandbox engagement via doctor pre-flight, dependency PATH check, settings.json pre-flight, and CC smoke test with failIfUnavailable:true exit-code proof

---

## v2.5 RightCron Reliability (Shipped: 2026-03-31)

**Phases:** 1 complete, 1 cancelled | **Plans:** 2 | **Code changes:** 3 files, +53/-8 lines

**Key accomplishments:**

- Replaced Agent-tool-delegated startup_prompt with inline /rightcron bootstrap so CronCreate is accessible on the main thread
- Restructured cronsync SKILL.md with CRITICAL guard and CHECK/RECONCILE phase split to prevent silent reconciler failures from background-agent delegation

**Known Gaps:**

- VER-01: End-to-end verification cancelled — Phase 22 skipped, user chose new milestone approach

---

## v2.4 Sandbox Telegram Fix (Shipped: 2026-03-28)

**Phases completed:** 1 phases, 1 plans, 1 tasks

**Key accomplishments:**

- CC event loop stall diagnosed: iv6() callback never calls M6() when Z===null after SubagentStop, leaving Telegram channel messages queued in hz indefinitely

**Known Gaps (deferred to CC fix):**

- FIX-01, FIX-02, VERIFY-01: Fix and verify Telegram response — deferred pending CC upstream fix for iv6/M6 channels bug. SEED-011 tracks the workaround.

---

## v2.3 Memory System (Shipped: 2026-03-27)

**Phases completed:** 4 phases, 9 plans, 16 tasks

**Key accomplishments:**

- SQLite memory module with WAL mode, FTS5 virtual table, append-only audit log via ABORT triggers, and rusqlite_migration 2.5 schema versioning — 9 tests all passing
- Removed memory_path from AgentDef and all struct literal sites (11 files), and confirmed default start_prompt is already "You are starting." — SEC-02 enforced architecturally
- Task 1:
- Injection-guarded SQLite CRUD layer (store/recall/search/forget) with FTS5 BM25 search, soft-delete audit trail, and 44 passing tests against real SQLite
- rmcp 1.3 stdio MCP server with 4 tools (store/recall/search/forget) wired to Phase 17 SQLite layer, per-agent .mcp.json codegen, and default start_prompt updated with tool references
- Three CLI-facing store functions (list_memories, search_memories_paged, hard_delete_memory) + serde::Serialize on MemoryEntry + full mod.rs re-exports — data layer ready for CLI inspection commands
- `rightclaw memory` subcommand group with list/search/delete/stats — operators can inspect any agent's SQLite memory database from the terminal without entering an agent session
- D-01 — Telegram detection fix (shell_wrapper.rs + settings.rs):

---

## v2.1 Headless Agent Isolation (Shipped: 2026-03-25)

**Phases completed:** 3 phases, 5 plans, 10 tasks

**Key accomplishments:**

- Shell wrapper sets HOME to agent dir with git/SSH/API key forwarding; per-agent .claude.json trust generation and credential symlink wired into cmd_up and init
- Absolute denyRead paths via host_home parameter, allowRead for agent dir, SandboxOverrides.allow_read, and integration tests covering Plan 01 artifacts
- Extended AgentConfig with three Telegram Option fields and extracted two codegen functions (generate_telegram_channel_config, install_builtin_skills) with 14 new tests covering all behaviors
- Wired git init, Telegram channel config, built-in skills reinstall, and settings.local.json pre-creation into cmd_up per-agent loop, and added git Warn check to doctor
- `rightclaw config strict-sandbox` writes /etc/claude-code/managed-settings.json with `allowManagedDomainsOnly:true`; doctor warns when file exists with rich or generic detail depending on content

---

## v2.0 Native Sandbox & Agent Isolation (Shipped: 2026-03-24)

**Phases completed:** 3 phases, 6 plans, 10 tasks

**Key accomplishments:**

- Stripped all OpenShell code paths -- sandbox.rs replaced by state.rs, policy.yaml removed from init/discovery/doctor, shell wrapper uses single direct-claude path
- v1 backward compatibility test added, all 48 relevant tests pass with zero openshell/sandbox references in codebase
- generate_settings() producing per-agent sandbox JSON with filesystem/network restrictions, security denyRead defaults, and user override merging via SandboxOverrides
- Wired generate_settings() into cmd_up() per-agent loop and refactored init.rs to delegate to shared codegen -- single source of truth for .claude/settings.json
- Linux-specific bwrap/socat binary detection and bwrap smoke test with AppArmor diagnostics in rightclaw doctor
- Replace OpenShell installation with bubblewrap + socat Linux deps and macOS Seatbelt early-return in install.sh

---
