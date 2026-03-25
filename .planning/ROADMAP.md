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

## Current Milestone: v3.0

(No active milestone — run `/gsd:new-milestone` to define next)
