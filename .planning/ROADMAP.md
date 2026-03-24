# Roadmap: RightClaw

## Completed Milestones

- **v1.0** (2026-03-21 -> 2026-03-23) -- Multi-agent runtime: Rust CLI, process-compose orchestration, OpenShell sandboxing, Telegram channels, skills.sh integration, RightCron scheduling. [Full roadmap](milestones/v1.0-ROADMAP.md)

## Current Milestone: v2.0 Native Sandbox & Agent Isolation

**Goal:** Replace OpenShell with Claude Code's native sandboxing (bubblewrap/Seatbelt). Remove all OpenShell code, generate per-agent `.claude/settings.json` with sandbox config, update tooling for new dependency profile.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [ ] **Phase 5: Remove OpenShell** - Strip all OpenShell code paths, simplify shell wrappers and runtime lifecycle
- [ ] **Phase 6: Sandbox Configuration** - Generate per-agent settings.json with CC native sandbox config and user overrides
- [ ] **Phase 7: Platform Compatibility** - Update doctor and installer for bubblewrap/socat on Linux, drop OpenShell checks

## Phase Details

### Phase 5: Remove OpenShell
**Goal**: Agents launch via direct `claude` invocation instead of OpenShell sandbox wrappers -- no OpenShell dependency required
**Depends on**: Phase 4 (v1.0)
**Requirements**: SBMG-01, SBMG-02, SBMG-03, SBMG-04, SBMG-05
**Success Criteria** (what must be TRUE):
  1. `rightclaw up` launches agents without OpenShell installed on the system
  2. Shell wrappers invoke `claude` directly -- no `openshell sandbox create` wrapping
  3. `rightclaw down` shuts down process-compose only -- no sandbox destroy step
  4. No OpenShell references remain in the codebase (sandbox.rs lifecycle, policy.yaml handling, openshell dep checks)
  5. Default agent template does not include policy.yaml files
**Plans:** 2 plans

Plans:
- [x] 05-01-PLAN.md -- Remove all OpenShell production code (structs, functions, templates, wrappers)
- [x] 05-02-PLAN.md -- Update all tests for simplified OpenShell-free types and APIs

### Phase 6: Sandbox Configuration
**Goal**: Each agent launches with CC native sandbox enforced via generated settings.json -- filesystem and network restrictions scoped per agent
**Depends on**: Phase 5
**Requirements**: SBCF-01, SBCF-02, SBCF-03, SBCF-04, SBCF-05, SBCF-06
**Success Criteria** (what must be TRUE):
  1. `rightclaw up` generates `.claude/settings.json` in each agent directory with `sandbox.enabled: true`
  2. Generated settings include filesystem restrictions (allowWrite scoped to agent dir) and network restrictions (allowedDomains for required services)
  3. Generated settings set `allowUnsandboxedCommands: false` and `autoAllowBashIfSandboxed: true` as secure defaults
  4. User can define sandbox overrides in `agent.yaml` (allowWrite, allowedDomains, excludedCommands) that merge with generated defaults
**Plans:** 2 plans

Plans:
- [x] 06-01-PLAN.md -- Create SandboxOverrides type and codegen/settings.rs module with generate_settings()
- [x] 06-02-PLAN.md -- Wire generate_settings() into cmd_up() and refactor init.rs

### Phase 7: Platform Compatibility
**Goal**: Users on Linux and macOS get correct dependency guidance and automated installation for the new sandbox stack
**Depends on**: Phase 6
**Requirements**: PLAT-01, PLAT-02, PLAT-03, PLAT-04, PLAT-05
**Success Criteria** (what must be TRUE):
  1. `rightclaw doctor` checks for bubblewrap and socat on Linux, skips these checks on macOS
  2. `rightclaw doctor` detects Ubuntu 24.04+ AppArmor restriction on unprivileged user namespaces and prints fix guidance
  3. `rightclaw doctor` no longer checks for OpenShell
  4. `install.sh` installs bubblewrap and socat on Linux (apt/dnf/pacman), skips on macOS, and no longer installs OpenShell
**Plans:** 2 plans

Plans:
- [ ] 07-01-PLAN.md -- Add bwrap/socat binary checks and bwrap smoke test to doctor.rs
- [x] 07-02-PLAN.md -- Replace OpenShell install with sandbox deps in install.sh

## Progress

**Execution Order:**
Phases execute in numeric order: 5 -> 6 -> 7

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 5. Remove OpenShell | v2.0 | 2/2 | Complete | 2026-03-24 |
| 6. Sandbox Configuration | v2.0 | 0/2 | Planning complete | - |
| 7. Platform Compatibility | v2.0 | 0/2 | Planning complete | - |
