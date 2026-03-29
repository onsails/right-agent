# Roadmap: RightClaw

## Milestones

- ✅ **v1.0 Core Runtime** - Phases 1-4 (shipped 2026-03-23)
- ✅ **v2.0 Native Sandbox** - Phases 5-7 (shipped 2026-03-24)
- ✅ **v2.1 Headless Agent Isolation** - Phases 8-10 (shipped 2026-03-25)
- ✅ **v2.2 Skills Registry** - Phases 11-15 (shipped 2026-03-26)
- ✅ **v2.3 Memory System** - Phases 16-19 (shipped 2026-03-27)
- ✅ **v2.4 Sandbox Telegram Fix** - Phase 20 (shipped 2026-03-28)
- 🔄 **v2.5 RightCron Reliability** - Phases 21-22 (active)

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

---

## v2.5 RightCron Reliability

- [x] **Phase 21: Bootstrap Fix + Reconciler Redesign** — Remove Agent tool delegation from startup_prompt; redesign rightcron SKILL.md with CHECK/RECONCILE split (completed 2026-03-29)
- [ ] **Phase 22: End-to-End Verification** — Manual test confirming reconciler boots, fires, and manages jobs correctly

## Phase Details

### Phase 21: Bootstrap Fix + Reconciler Redesign
**Goal**: rightcron boots inline in the main thread and reconciles cron jobs without Agent tool delegation
**Depends on**: Nothing (first phase of milestone)
**Requirements**: BOOT-01, BOOT-02, RECON-01, RECON-02
**Success Criteria** (what must be TRUE):
  1. `startup_prompt` in `shell_wrapper.rs` no longer contains "Use the Agent tool to run this in the background:" — rightcron executes inline
  2. After `rightclaw up`, `CronList` confirms a `*/5 * * * *` reconciler job exists in the agent session
  3. rightcron SKILL.md has distinct CHECK and RECONCILE commands — CHECK outputs a structured diff with no side effects, RECONCILE calls CronCreate/CronDelete directly in the main thread
  4. Neither CHECK nor RECONCILE delegates to a background Agent tool at any point
**Plans**: 2 plans
Plans:
- [ ] 21-01-PLAN.md — TDD fix: startup_prompt regression tests + inline bootstrap constant (BOOT-01, BOOT-02)
- [ ] 21-02-PLAN.md — SKILL.md restructure: CRITICAL guard + CHECK/RECONCILE split + workspace rebuild (RECON-01, RECON-02)

### Phase 22: End-to-End Verification
**Goal**: Confirmed working — reconciler job boots, fires, and correctly manages user-defined cron specs
**Depends on**: Phase 21
**Requirements**: VER-01
**Success Criteria** (what must be TRUE):
  1. A `crons/*.yaml` spec file placed in an agent dir results in the corresponding job appearing in `CronList` after the reconciler fires (or is triggered manually)
  2. Removing the spec file and triggering reconcile results in the job being deleted from `CronList`
  3. No Agent tool delegation appears in the reconciler execution trace
**Plans**: TBD

## Progress Table

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 21. Bootstrap Fix + Reconciler Redesign | 0/2 | Complete    | 2026-03-29 |
| 22. End-to-End Verification | 0/1 | Not started | - |
