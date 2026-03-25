# Roadmap: RightClaw

## Completed Milestones

- **v1.0** (2026-03-21 -> 2026-03-23) -- Multi-agent runtime: Rust CLI, process-compose orchestration, OpenShell sandboxing, Telegram channels, skills.sh integration, RightCron scheduling. [Full roadmap](milestones/v1.0-ROADMAP.md)

<details>
<summary>v2.0 Native Sandbox & Agent Isolation (Shipped: 2026-03-24)</summary>

### Phase 5: Remove OpenShell
**Goal**: Agents launch via direct `claude` invocation instead of OpenShell sandbox wrappers
**Plans:** 2 plans (complete)

### Phase 6: Sandbox Configuration
**Goal**: Each agent launches with CC native sandbox enforced via generated settings.json
**Plans:** 2 plans (complete)

### Phase 7: Platform Compatibility
**Goal**: Users on Linux and macOS get correct dependency guidance and automated installation for the new sandbox stack
**Plans:** 2 plans (complete)

</details>

- **v2.1** (2026-03-24 -> 2026-03-25) -- Headless Agent Isolation: per-agent HOME override, credential symlinks, git/SSH env forwarding, pre-populated .claude/ scaffold, git init, Telegram channel copy, managed-settings doctor check. [Full roadmap](milestones/v2.1-ROADMAP.md)

## Current Milestone: v2.2 Skills Registry

**Milestone Goal:** Replace ClawHub with skills.sh as the primary skill registry, ship `/skills` skill manager, and add per-agent env var injection via `agent.yaml`.

### Phases

- [ ] **Phase 11: Env Var Injection** - Per-agent env vars declared in agent.yaml, shell-quoted and injected before `exec claude`
- [ ] **Phase 12: Skills Registry Rename** - `/clawhub` replaced by `/skills` (skills.sh primary, ClawHub removed completely), stale dirs cleaned
- [ ] **Phase 13: Policy Gate Rework** - `/skills` SKILL.md policy gate rewritten for CC-native sandbox; drops all OpenShell/policy.yaml references

## Phase Details

### Phase 11: Env Var Injection
**Goal**: Users can declare per-agent env vars in agent.yaml that are safely injected into the agent's shell environment on every `rightclaw up`
**Depends on**: Nothing (v2.1 complete)
**Requirements**: ENV-01, ENV-02, ENV-03, ENV-04, ENV-05
**Success Criteria** (what must be TRUE):
  1. Adding `env: {MY_VAR: "hello world"}` to agent.yaml causes `export MY_VAR='hello world'` to appear in the generated shell wrapper before `exec claude`
  2. Values with spaces, quotes, and special shell characters do not break wrapper syntax or inject unintended commands
  3. Env vars appear in the wrapper before `export HOME=` so identity vars cannot shadow them
  4. `installed.json` is created on first `rightclaw up` but not overwritten on subsequent runs — user-installed skill state persists across restarts
  5. The generated `agent.yaml` template includes a comment warning that `env:` values are stored in plaintext and must not contain secrets
**Plans**: TBD

### Phase 12: Skills Registry Rename
**Goal**: The `/clawhub` skill and all ClawHub references are replaced by `/skills` backed by skills.sh; existing agent dirs are cleaned of stale clawhub directories
**Depends on**: Phase 11
**Requirements**: SKILLS-01, SKILLS-02, SKILLS-03, SKILLS-04, SKILLS-05
**Success Criteria** (what must be TRUE):
  1. No `clawhub` directory, file, or `SKILL_CLAWHUB` constant exists anywhere in the codebase
  2. `rightclaw init` and `rightclaw up` install a `/skills` skill into each agent's `.claude/skills/` directory
  3. The `/skills` skill uses `npx skills find <query>` for search and `npx skills add <owner>/<repo> -a claude` for install
  4. `rightclaw up` removes `.claude/skills/clawhub/` from existing agent dirs if present
**Plans**: TBD

### Phase 13: Policy Gate Rework
**Goal**: The `/skills` SKILL.md policy gate reflects CC-native sandbox reality — no OpenShell references, instructs agent to check `settings.json` capabilities before activating a skill
**Depends on**: Phase 12
**Requirements**: GATE-01, GATE-02
**Success Criteria** (what must be TRUE):
  1. `/skills` SKILL.md contains no references to `policy.yaml`, `metadata.openclaw.requires`, or OpenShell
  2. `/skills` SKILL.md instructs the agent to read `settings.json` `allowedDomains` and `allowWrite` and verify skill network/filesystem requirements are satisfied before activation
**Plans**: TBD

## Progress

**Execution Order:** 11 → 12 → 13

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 11. Env Var Injection | v2.2 | 0/? | Not started | - |
| 12. Skills Registry Rename | v2.2 | 0/? | Not started | - |
| 13. Policy Gate Rework | v2.2 | 0/? | Not started | - |
