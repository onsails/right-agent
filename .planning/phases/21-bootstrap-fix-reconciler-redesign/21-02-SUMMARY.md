---
phase: 21-bootstrap-fix-reconciler-redesign
plan: "02"
subsystem: skills/cronsync
tags: [skill, rightcron, reconciler, cron, reliability]
dependency_graph:
  requires: []
  provides: [RECON-01, RECON-02]
  affects: [skills/cronsync/SKILL.md, crates/rightclaw/src/codegen/skills.rs]
tech_stack:
  added: []
  patterns: [skill-restructure, CHECK-RECONCILE-split]
key_files:
  created: []
  modified:
    - skills/cronsync/SKILL.md
decisions:
  - "CRITICAL guard placed at top of Reconciliation Algorithm section (blockquote format) — visible before any step is read"
  - "Step A (CHECK) scoped to read-only operations (CronList allowed); Step B (RECONCILE) holds all CronCreate/CronDelete calls"
  - "Bootstrap step 2 annotated inline with blockquote reminder — same pattern reinforces constraint at the call site"
metrics:
  duration: "106s"
  completed_date: "2026-03-29"
  tasks_completed: 2
  files_modified: 1
---

# Phase 21 Plan 02: Reconciler Redesign — CRITICAL Guard + CHECK/RECONCILE Split Summary

**One-liner:** Restructured cronsync SKILL.md with an Agent-tool CRITICAL guard and explicit CHECK/RECONCILE phase split to prevent silent reconciler failures from background-agent delegation.

## Objective

Prevent the LLM from delegating CronCreate/CronDelete to a background Agent tool, which silently fails because those tools are main-thread-only. The structural CHECK/RECONCILE split makes the constraint visible and enforced by algorithm structure.

## Tasks Completed

| # | Task | Commit | Files |
|---|------|--------|-------|
| 1 | Restructure SKILL.md — CRITICAL guard + CHECK/RECONCILE split | 3c0de2b | skills/cronsync/SKILL.md |
| 2 | Rebuild workspace to embed updated SKILL.md | (no new files) | — |

## Changes Made

### skills/cronsync/SKILL.md

Three targeted changes per plan spec:

**Change 1 — Bootstrap step 2 annotation (D-06):**
Added inline blockquote after step 2 bullet content:
> Call CronCreate directly in this turn — do NOT use the Agent tool.

**Change 2 — CRITICAL guard before reconciliation algorithm (D-05):**
Inserted blockquote at top of `## Reconciliation Algorithm` section, before any step is encountered. Guard text explicitly names CronCreate, CronDelete, CronList, explains why delegation fails (ToolSearch doesn't find them in background agent), and mandates direct-thread calls.

**Change 3 — CHECK/RECONCILE split (D-03, D-04):**
- Added `### Step A: Compute diff (CHECK)` with a no-create/delete constraint statement
- Demoted Steps 1-3 to `####` headings under Step A
- Added `#### Diff computation` after Step 3 with explicit new/changed/unchanged/orphaned diff categories
- Added `### Step B: Apply changes (RECONCILE)` with direct-call reminder statement
- Demoted Steps 4-6 to `####` headings under Step B
- Step 2 note clarified: CronList is a READ-ONLY call, allowed in CHECK

## Verification

```
rg "CRITICAL: NEVER use the Agent tool" skills/cronsync/SKILL.md  ✓
rg "Step A: Compute diff" skills/cronsync/SKILL.md                ✓
rg "Step B: Apply changes" skills/cronsync/SKILL.md               ✓
rg "do NOT use the Agent tool" skills/cronsync/SKILL.md           ✓
cargo build --workspace                                            ✓ Finished dev profile in 0.16s
```

## Requirements Satisfied

- **RECON-01**: CHECK/RECONCILE split in skill source — Step A (read-only diff) and Step B (direct cron tool calls) are structurally separate sections
- **RECON-02**: CRITICAL guard prevents Agent tool delegation at the algorithm entry point AND at the bootstrap call site

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None.

## Self-Check: PASSED

- skills/cronsync/SKILL.md: FOUND
- Commit 3c0de2b: FOUND (git log verified)
- All four verification grep patterns: FOUND
- cargo build --workspace: PASSED
