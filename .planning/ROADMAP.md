# Roadmap: RightClaw

## Milestones

- ✅ **v1.0 Core Runtime** - Phases 1-4 (shipped 2026-03-23)
- ✅ **v2.0 Native Sandbox** - Phases 5-7 (shipped 2026-03-24)
- ✅ **v2.1 Headless Agent Isolation** - Phases 8-10 (shipped 2026-03-25)
- ✅ **v2.2 Skills Registry** - Phases 11-15 (shipped 2026-03-26)
- ✅ **v2.3 Memory System** - Phases 16-19 (shipped 2026-03-27)
- ✅ **v2.4 Sandbox Telegram Fix** - Phase 20 (shipped 2026-03-28)
- ✅ **v2.5 RightCron Reliability** - Phase 21 (shipped 2026-03-31)
- ✅ **v3.0 Teloxide Bot Runtime** - Phases 22-28.2 (shipped 2026-04-01)
- ✅ **v3.1 Sandbox Fix & Verification** - Phases 29-31 (shipped 2026-04-03)
- ✅ **v3.2 MCP & Tunnel** - Phases 38-41 (shipped 2026-04-05)
- ✅ **v3.3 MCP Self-Management** - Phase 1 (shipped 2026-04-06)
- 📋 **v3.4 Chrome Integration** - Phases 42-44 (planned)

## Phases

<details>
<summary>✅ v1.0 Core Runtime (Phases 1-4) - SHIPPED 2026-03-23</summary>

See [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.0 Native Sandbox (Phases 5-7) - SHIPPED 2026-03-24</summary>

See [milestones/v2.0-ROADMAP.md](milestones/v2.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.1 Headless Agent Isolation (Phases 8-10) - SHIPPED 2026-03-25</summary>

See [milestones/v2.1-ROADMAP.md](milestones/v2.1-ROADMAP.md)

</details>

<details>
<summary>✅ v2.2 Skills Registry (Phases 11-15) - SHIPPED 2026-03-26</summary>

See [milestones/v2.2-ROADMAP.md](milestones/v2.2-ROADMAP.md)

</details>

<details>
<summary>✅ v2.3 Memory System (Phases 16-19) — SHIPPED 2026-03-27</summary>

See [milestones/v2.3-ROADMAP.md](milestones/v2.3-ROADMAP.md)

</details>

<details>
<summary>✅ v2.4 Sandbox Telegram Fix (Phase 20) — SHIPPED 2026-03-28</summary>

See [milestones/v2.4-ROADMAP.md](milestones/v2.4-ROADMAP.md)

</details>

<details>
<summary>✅ v2.5 RightCron Reliability (Phase 21) — SHIPPED 2026-03-31</summary>

See [milestones/v2.5-ROADMAP.md](milestones/v2.5-ROADMAP.md)

</details>

<details>
<summary>✅ v3.0 Teloxide Bot Runtime (Phases 22-28.2) — SHIPPED 2026-04-01</summary>

See [milestones/v3.0-ROADMAP.md](milestones/v3.0-ROADMAP.md)

</details>

<details>
<summary>✅ v3.1 Sandbox Fix & Verification (Phases 29-31) — SHIPPED 2026-04-03</summary>

See [milestones/v3.1-ROADMAP.md](milestones/v3.1-ROADMAP.md)

</details>

<details>
<summary>✅ v3.2 MCP & Tunnel (Phases 38-41) — SHIPPED 2026-04-05</summary>

See [milestones/v3.2-ROADMAP.md](milestones/v3.2-ROADMAP.md)

</details>

<details>
<summary>✅ v3.3 MCP Self-Management (Phase 1) — SHIPPED 2026-04-06</summary>

See [milestones/v3.3-ROADMAP.md](milestones/v3.3-ROADMAP.md)

</details>

### 📋 v3.4 Chrome Integration (Planned)

**Milestone Goal:** Wire `chrome-devtools-mcp` into rightclaw as a built-in browser MCP — auto-detected at init, injected into every agent's `.mcp.json` and sandbox settings, validated by doctor and bot startup, and surfaced in AGENTS.md system prompt templates.

- [x] **Phase 42: Chrome Config Infrastructure + MCP Injection** - ChromeConfig struct, per-agent .mcp.json injection, sandbox overrides (completed 2026-04-06)
- [ ] **Phase 43: Init Detection + Up Revalidation** - Auto-detect Chrome at init, --chrome-path override, revalidate on every up
- [ ] **Phase 44: Validation + AGENTS.md Template** - Doctor check, bot startup warn, AGENTS.md browser automation section

## Phase Details

### Phase 42: Chrome Config Infrastructure + MCP Injection
**Goal**: Per-agent `.mcp.json` carries a working `chrome-devtools` entry on every `rightclaw up` when Chrome is configured
**Depends on**: Phase 1 (v3.3)
**Requirements**: INJECT-01, INJECT-02, SBOX-01, SBOX-02
**Success Criteria** (what must be TRUE):
  1. After `rightclaw up`, each agent's `.mcp.json` contains a `chrome-devtools` entry with the configured chrome path, `--headless`, `--isolated`, `--no-sandbox`, and `--userDataDir <agent_dir>/.chrome-profile` args
  2. The `chrome-devtools` entry uses an absolute binary path to `chrome-devtools-mcp` — no `npx` in the command
  3. The agent's `settings.json` includes the Chrome binary in `allowedCommands` and the `.chrome-profile` dir in `allowWrite`
  4. Chrome sandbox overrides merge additively with existing `SandboxOverrides` from `agent.yaml` — existing overrides are not clobbered
**Plans**: 3 plans
Plans:
- [x] 42-01-PLAN.md — ChromeConfig struct + read/write support in config.rs
- [x] 42-02-PLAN.md — chrome-devtools MCP injection + sandbox override generators
- [x] 42-03-PLAN.md — cmd_up() wiring: hoist global_cfg, pass chrome_cfg to both generators

### Phase 43: Init Detection + Up Revalidation
**Goal**: Chrome path is discovered at init and revalidated silently on every `rightclaw up` — operators never lose injection silently
**Depends on**: Phase 42
**Requirements**: CHROME-01, CHROME-02, CHROME-03, INJECT-03
**Success Criteria** (what must be TRUE):
  1. Running `rightclaw init` on a machine with Chrome at a standard path saves `chrome.chrome_path` to `~/.rightclaw/config.yaml` automatically
  2. Running `rightclaw init --chrome-path /custom/chrome` saves the provided path to config regardless of what auto-detection finds
  3. Running `rightclaw init` on a machine with no Chrome logs a warning and completes normally — init does not fail
  4. Running `rightclaw up` when the configured Chrome path no longer exists logs a warning and skips injection for that run — agents start normally
**Plans**: 2 plans
Plans:
- [ ] 43-01-PLAN.md — Chrome + MCP detection helpers, --chrome-path arg, cmd_init() single-write refactor
- [ ] 43-02-PLAN.md — cmd_up() per-run Chrome path revalidation

### Phase 44: Validation + AGENTS.md Template
**Goal**: Operators can verify Chrome configuration via `rightclaw doctor`; agents know to use ChromeDevTools MCP for browser tasks
**Depends on**: Phase 43
**Requirements**: VALID-01, VALID-02, AGENT-01
**Success Criteria** (what must be TRUE):
  1. `rightclaw doctor` output includes a Chrome check — Warn when Chrome is configured but binary is absent or path is unconfigured
  2. Bot process startup emits `tracing::warn!` when Chrome is configured but binary is missing; emits `tracing::debug!` when Chrome is not configured at all
  3. Agent AGENTS.md templates contain a "Browser Automation" section instructing use of ChromeDevTools MCP with `navigate_page` → `take_snapshot` → uid-based interaction → `take_screenshot` verification workflow
**Plans**: TBD

## Progress

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1-4. Core Runtime | v1.0 | ✓ | Complete | 2026-03-23 |
| 5-7. Native Sandbox | v2.0 | ✓ | Complete | 2026-03-24 |
| 8-10. Headless Agent Isolation | v2.1 | ✓ | Complete | 2026-03-25 |
| 11-15. Skills Registry | v2.2 | ✓ | Complete | 2026-03-26 |
| 16-19. Memory System | v2.3 | ✓ | Complete | 2026-03-27 |
| 20. Sandbox Telegram Fix | v2.4 | ✓ | Complete | 2026-03-28 |
| 21. RightCron Reliability | v2.5 | ✓ | Complete | 2026-03-31 |
| 22-28.2. Teloxide Bot Runtime | v3.0 | ✓ | Complete | 2026-04-01 |
| 29-31. Sandbox Fix & Verification | v3.1 | ✓ | Complete | 2026-04-03 |
| 38-41. MCP & Tunnel | v3.2 | ✓ | Complete | 2026-04-05 |
| 1. MCP management tools | v3.3 | 2/2 | Complete | 2026-04-06 |
| 42. Chrome Config + MCP Injection | v3.4 | 3/3 | Complete   | 2026-04-06 |
| 43. Init Detection + Up Revalidation | v3.4 | 0/2 | Not started | - |
| 44. Validation + AGENTS.md Template | v3.4 | 0/TBD | Not started | - |
