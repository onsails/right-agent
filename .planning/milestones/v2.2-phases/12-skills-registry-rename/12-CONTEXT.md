# Phase 12: Skills Registry Rename - Context

**Gathered:** 2026-03-25
**Status:** Ready for planning

<domain>
## Phase Boundary

Rename all `clawhub` references to `skills` throughout the codebase: directory (`skills/clawhub/` → `skills/skills/`), Rust constant (`SKILL_CLAWHUB` → `SKILL_SKILLS`), install path in `install_builtin_skills()` (`clawhub/SKILL.md` → `skills/SKILL.md`), all tests, and the `init.rs` print statement. Add silent stale-dir cleanup in `rightclaw up`. No new commands, no policy gate changes (deferred to Phase 13).

The SKILL.md content is already written correctly (uses `npx skills`, skills.sh) — it just lives in the wrong directory.

</domain>

<decisions>
## Implementation Decisions

### Stale Dir Cleanup (SKILLS-05)
- **D-01:** `rightclaw up` removes `.claude/skills/clawhub/` from existing agent dirs silently: `fs::remove_dir_all(...)` with error ignored (non-fatal). Not prod — no need for logging, retries, or failure propagation on cleanup.
- **D-02:** Cleanup runs every `up` call unconditionally (idempotent). No "first run after upgrade" tracking needed.

### Policy Gate in SKILL.md (GATE-01, GATE-02)
- **D-03:** The `metadata.openclaw.requires` table in the current SKILL.md policy gate section is **NOT touched in Phase 12**. Phase 13 rewrites the entire section — no half-cleanup.

### Rename Scope
- **D-04:** All locations referencing `clawhub` in test assertions, print statements, include_str! paths, and skill install paths are updated atomically in a single plan.
- **D-05:** The `skills/clawhub/` source directory is renamed to `skills/skills/`. The `SKILL_CLAWHUB` constant is renamed to `SKILL_SKILLS`.

### Claude's Discretion
- Whether to use `fs::remove_dir_all().ok()` or a `if path.exists()` guard for D-01 stale cleanup — either is fine.
- How many plans to split this into (likely one — it's all mechanical rename).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` §Skills Registry — SKILLS-01 through SKILLS-05

### Files to rename/update (read all before touching)
- `skills/clawhub/SKILL.md` — current skill content to be moved to `skills/skills/SKILL.md` (already has correct content)
- `crates/rightclaw/src/codegen/skills.rs` — `SKILL_CLAWHUB` constant, `install_builtin_skills()`, all tests
- `crates/rightclaw/src/init.rs` — `install_builtin_skills()` call site, print statement line 173, test assertion line 250
- `crates/rightclaw-cli/src/main.rs` — `install_builtin_skills()` call site, test line 755
- `crates/rightclaw-cli/tests/cli_integration.rs` — may reference `clawhub/SKILL.md`
- `crates/rightclaw-cli/tests/home_isolation.rs` — may reference `clawhub/SKILL.md`

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `skills/cronsync/SKILL.md` — example of how a built-in skill is structured (reference for naming/path convention: `skills/<name>/SKILL.md`)
- Phase 9 `settings.local.json` create-if-absent pattern (already applied to installed.json in Phase 11) — reference for `fs::remove_dir_all().ok()` style error handling

### Established Patterns
- `include_str!("../../../../skills/<name>/SKILL.md")` — relative path from `crates/rightclaw/src/codegen/skills.rs`
- `built_in_skills: &[(&str, &str)]` slice in `install_builtin_skills()` — add/rename entries here
- FAIL FAST: stale cleanup is the only place where errors are intentionally ignored (it's a best-effort cleanup, not a data operation)

### Integration Points
- `install_builtin_skills(&agent_path)` is called from both `init.rs` and `main.rs` (cmd_up) — one change in `skills.rs` propagates everywhere
- Stale cleanup belongs in `cmd_up` in `main.rs` (same place init is called), NOT in `install_builtin_skills()` — keeps the function focused

</code_context>

<specifics>
## Specific Ideas

- Stale cleanup: `let _ = std::fs::remove_dir_all(agent.path.join(".claude/skills/clawhub"))` — one line per agent in the `cmd_up` loop, before or after `install_builtin_skills`

</specifics>

<deferred>
## Deferred Ideas

- `metadata.openclaw.requires` table cleanup — deferred to Phase 13 (GATE-01, GATE-02)
- Verbose stale cleanup logging — not needed, not prod

</deferred>

---

*Phase: 12-skills-registry-rename*
*Context gathered: 2026-03-25*
