# Roadmap: RightClaw

## Milestones

- ✅ **v1.0 Core Runtime** - Phases 1-4 (shipped 2021-03-23)
- ✅ **v2.0 Native Sandbox** - Phases 5-7 (shipped 2021-03-24)
- ✅ **v2.1 Headless Agent Isolation** - Phases 8-10 (shipped 2021-03-25)
- ✅ **v2.2 Skills Registry** - Phases 11-15 (shipped 2021-03-26)
- ✅ **v2.3 Memory System** - Phases 16-19 (shipped 2021-03-27)
- 🔄 **v2.4 Sandbox Telegram Fix** - Phases 20-22 (active)

## Phases

<details>
<summary>✅ v1.0 Core Runtime (Phases 1-4) - SHIPPED 2021-03-23</summary>

See [milestones/v1.0-ROADMAP.md](milestones/v1.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.0 Native Sandbox (Phases 5-7) - SHIPPED 2021-03-24</summary>

See [milestones/v2.0-ROADMAP.md](milestones/v2.0-ROADMAP.md)

</details>

<details>
<summary>✅ v2.1 Headless Agent Isolation (Phases 8-10) - SHIPPED 2021-03-25</summary>

See [milestones/v2.1-ROADMAP.md](milestones/v2.1-ROADMAP.md)

</details>

<details>
<summary>✅ v2.2 Skills Registry (Phases 11-15) - SHIPPED 2021-03-26</summary>

See [milestones/v2.2-ROADMAP.md](milestones/v2.2-ROADMAP.md)

</details>

<details>
<summary>✅ v2.3 Memory System (Phases 16-19) — SHIPPED 2021-03-27</summary>

See [milestones/v2.3-ROADMAP.md](milestones/v2.3-ROADMAP.md)

</details>

---

## v2.4 Sandbox Telegram Fix

- [x] **Phase 20: Diagnosis** — Identify root cause of Telegram silence under CC sandbox (completed 2021-03-28)

---

## Phase Details

### Phase 20: Diagnosis
**Goal**: Root cause of CC sandbox blocking Telegram event processing is identified and confirmed
**Depends on**: Nothing (investigation phase)
**Requirements**: DIAG-01, DIAG-02, DIAG-03
**Success Criteria** (what must be TRUE):
  1. Developer can point to specific log lines in right-debug.log that show where Telegram event processing stops under sandbox
  2. A log comparison between sandbox-on and --no-sandbox runs exists that confirms the failure is sandbox-specific
  3. The specific config element responsible is named (bwrap network rule, socat relay gap, or settings.json network/filesystem section)
  4. A written diagnosis note exists summarizing root cause and proposed fix approach
**Plans**: 1 plan
Plans:
- [x] 20-01-PLAN.md — Write DIAGNOSIS.md synthesizing all evidence into root cause and fix proposal
