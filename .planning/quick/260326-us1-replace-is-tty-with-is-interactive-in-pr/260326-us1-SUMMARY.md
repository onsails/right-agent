---
phase: 260326-us1
plan: 01
subsystem: templates, planning-docs
tags: [process-compose, template, field-rename, docs]
dependency_graph:
  requires: []
  provides: [correct-is-interactive-field-in-template]
  affects: [generated-process-compose.yaml, rightclaw-restart]
tech_stack:
  added: []
  patterns: []
key_files:
  created: []
  modified:
    - templates/process-compose.yaml.j2
    - .planning/PROJECT.md
    - .planning/milestones/v2.2-ROADMAP.md
    - .planning/SESSION_REPORT.md
decisions:
  - "is_interactive is the correct process-compose v1.x field name; is_tty was a wrong guess"
  - "restart status left as 'unverified' rather than 'fixed' — requires live test to confirm"
metrics:
  duration: ~5min
  completed: 2026-03-26
  tasks_completed: 2
  files_modified: 4
---

# Quick Task 260326-us1: Replace is_tty with is_interactive in process-compose Template

**One-liner:** Replaced `is_tty: true` with `is_interactive: true` (correct field name) in the process-compose template and updated all planning docs referencing the old field or the associated restart bug.

## What Was Done

### Task 1: Update template field

Changed line 10 of `templates/process-compose.yaml.j2` from `is_tty: true` to `is_interactive: true`. This is the only source-of-truth for the generated `process-compose.yaml` — no Rust code changes required.

### Task 2: Update planning docs

Surgically updated three planning files:

- `.planning/PROJECT.md` — Known limitations entry changed from "disabled (is_tty bug)" to "status unknown — may now work" after the field rename
- `.planning/milestones/v2.2-ROADMAP.md` — Tech debt entry annotated with "field renamed to is_interactive; restart status unverified"
- `.planning/SESSION_REPORT.md` — Gotcha table rows for `is_tty` and PC restart crash updated; Key Learning #3 updated to reference `is_interactive`

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| 1 | b429bf8 | feat(260326-us1-01): replace is_tty with is_interactive in process-compose template |
| 2 | c0170b5 | docs(260326-us1-01): update planning docs to reflect is_tty -> is_interactive rename |

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None. This is a field rename + doc update only. The actual effect on `rightclaw restart` is explicitly left as "unverified" per plan — requires a live test against a running process-compose instance.

## Self-Check: PASSED

- `templates/process-compose.yaml.j2` contains `is_interactive: true`, no `is_tty` — VERIFIED
- All three planning files reference `is_interactive` — VERIFIED
- Both commits exist: b429bf8, c0170b5 — VERIFIED
