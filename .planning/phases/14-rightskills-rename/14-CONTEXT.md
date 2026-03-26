# Phase 14: rightskills Rename - Context

**Gathered:** 2026-03-26
**Status:** Ready for planning

<domain>
## Phase Boundary

Rename the `/skills` skill to `/rightskills` across all touch points: the source directory, the Rust constant and include path, the SKILL.md frontmatter `name:` field, and all `/skills` invocation examples in the SKILL.md body. No new functionality. No stale-dir cleanup (not in prod).

</domain>

<decisions>
## Implementation Decisions

### Rename Scope
- **D-01:** Source directory `skills/skills/` → `skills/rightskills/` (filesystem rename)
- **D-02:** Rust constant `SKILL_SKILLS` → `SKILL_RIGHTSKILLS` in `crates/rightclaw/src/codegen/skills.rs`
- **D-03:** `include_str!` path updated: `"../../../../skills/skills/SKILL.md"` → `"../../../../skills/rightskills/SKILL.md"`
- **D-04:** Install path entry updated: `("skills/SKILL.md", SKILL_RIGHTSKILLS)` → `("rightskills/SKILL.md", SKILL_RIGHTSKILLS)`
- **D-05:** All test assertions in `skills.rs` and `init.rs` that reference `.claude/skills/skills/SKILL.md` update to `.claude/skills/rightskills/SKILL.md`
- **D-06:** `init.rs` display text and path assertion updated (currently `agents/right/.claude/skills/skills/SKILL.md`)

### SKILL.md Changes
- **D-07:** SKILL.md frontmatter `name: skills` → `name: rightskills` (this is the agent invocation name change: `/skills` → `/rightskills`)
- **D-08:** All `/skills` invocation examples in the SKILL.md body text update to `/rightskills` (e.g. "invoke `/skills install`" → "invoke `/rightskills install`"). **IMPORTANT:** `skills.sh` domain references must NOT change — only slash command invocations like `/skills`, `/skills install`, `/skills list`, `/skills remove`, `/skills update`.
- **D-09:** The `### skill-doctor` section keeps its current name — `/skill-doctor` invocation is unchanged. No rename.

### Stale Dir Cleanup
- **D-10:** No stale cleanup for `.claude/skills/skills/` in `cmd_up`. Project is not in production — existing agent dirs do not need migration.

### Claude's Discretion
- Exact sed-style substitution strategy for D-08 (careful regex to match `/skills` command refs but not `skills.sh` URLs or prose words like "skills")
- Whether to use `git mv` or filesystem rename for the directory

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Files being modified
- `skills/skills/SKILL.md` — only source of truth for the skill; read in full before any edit; D-07 and D-08 changes here
- `crates/rightclaw/src/codegen/skills.rs` — D-02, D-03, D-04, D-05; also update all test assertions
- `crates/rightclaw/src/init.rs` — D-06; display text and path assertion

### Prior phase (reference for stale cleanup pattern — NOT to replicate this phase)
- `crates/rightclaw-cli/src/main.rs` — Phase 12 clawhub cleanup pattern; D-10 says this pattern is NOT applied in Phase 14

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- Stale cleanup pattern in `main.rs` (`let _ = std::fs::remove_dir_all(...)`) — established by Phase 12, but NOT used in Phase 14 per D-10

### Established Patterns
- `include_str!` macro with relative path from `skills.rs` — same macro, just update the path string
- Test assertions use `.join(".claude/skills/<name>/SKILL.md")` path style — update name segment only

### Integration Points
- `install_builtin_skills()` in `skills.rs` uses a `built_in_skills` slice of `(path, content)` tuples — update the path string in the tuple
- `init.rs` has a `println!` display line and a path assertion that reference `skills/SKILL.md` — both update

</code_context>

<specifics>
## Specific Ideas

- The rename in SKILL.md body (D-08) should be a targeted string replacement: only `/skills` (as slash command prefix) changes, not `skills.sh`, not the word "skills" in prose. Pattern: lines containing `` `/skills` `` or `` `/skills `` (backtick-prefixed slash commands) are the targets.

</specifics>

<deferred>
## Deferred Ideas

- Stale `.claude/skills/skills/` cleanup — explicitly deferred (not in prod, no need)
- `rightclaw up` / `rightclaw doctor` frontmatter/compatibility validation — Phase 15+
- Structured `metadata.requires.*` frontmatter extension — ecosystem uses prose only; revisit if skills.sh adds a standard

</deferred>

---

*Phase: 14-rightskills-rename*
*Context gathered: 2026-03-26*
