# Phase 4: Skills and Automation - Context

**Gathered:** 2026-03-22
**Status:** Ready for planning

<domain>
## Phase Boundary

Two Claude Code skills: `/clawhub` for skill management via ClawHub HTTP API, and `/cronsync` for declarative cron job reconciliation. Also: generated system prompt for CronSync bootstrap. These are SKILL.md files and markdown content, not Rust code (except minor shell wrapper template updates for system prompt).

</domain>

<decisions>
## Implementation Decisions

### ClawHub API
- **D-01:** Research actual ClawHub API endpoints during plan-phase research (not known yet)
- **D-02:** Existing `skills/clawhub/SKILL.md` is a good starting point — refine with real API details
- **D-03:** If ClawHub API is unreachable, skill suggests manual git clone as fallback

### Skill Install Path
- **D-04:** Skills install to `$RIGHTCLAW_HOME/agents/<agent>/skills/<name>/` — per-agent isolation
- **D-05:** No shared/global skill location in v1
- **D-06:** `skills/installed.json` tracks installed skills per agent

### Policy Gate
- **D-07:** When a skill requests permissions the agent's policy doesn't grant → BLOCK installation
- **D-08:** Show exactly what permissions are needed vs what's allowed — user must manually update policy.yaml first
- **D-09:** No auto-expansion of policy — security-first means user explicitly decides
- **D-10:** Gate reads `metadata.openclaw.requires` from SKILL.md frontmatter (bins, env vars, network)

### CronSync Design
- **D-11:** CronSync is a Claude Code skill (`/cronsync`) that uses CC's native CronCreate/CronList/CronDelete tools
- **D-12:** Reconciliation: reads `crons/*.yaml` as desired state, CronList as actual state, state.json as mapping
- **D-13:** Creates missing jobs, deletes orphaned jobs, recreates changed jobs (hash-based change detection)
- **D-14:** Lock-file concurrency: heartbeat-based with configurable TTL, UTC ISO 8601 timestamps
- **D-15:** Lock files at `crons/.locks/<name>.json`, state at `crons/state.json` — both gitignored

### CronSync Bootstrap
- **D-16:** RightClaw generates `$RIGHTCLAW_HOME/run/<agent>-system.md` with CronSync bootstrap instructions
- **D-17:** Shell wrapper passes this as additional `--append-system-prompt-file` — user can't modify (regenerated on each `up`)
- **D-18:** System prompt instructs agent to run `/cronsync` on startup if `crons/` directory exists with YAML specs
- **D-19:** The system prompt file is read OUTSIDE the sandbox by the wrapper script — no policy change needed

### Shell Wrapper Update
- **D-20:** Shell wrapper template needs updating to support second `--append-system-prompt-file` for system prompt
- **D-21:** Codegen (Rust) needs to generate the system prompt file at `run/<agent>-system.md`

### Claude's Discretion
- ClawHub API error handling and retry logic
- CronSync skill conversation style (verbose vs quiet)
- Lock file stale detection edge cases
- System prompt wording for CronSync bootstrap

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Existing skills
- `skills/clawhub/SKILL.md` — Current ClawHub skill (needs refinement with real API endpoints)

### CronSync design
- `seed.md` §CronSync — Full CronSync design: reconciliation logic, YAML format, lock files, state.json, concurrency control

### Codebase
- `templates/agent-wrapper.sh.j2` — Shell wrapper template (needs second --append-system-prompt-file)
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — Wrapper generation logic (needs system prompt file generation)
- `crates/rightclaw/src/codegen/process_compose.rs` — PC config generation (reference for codegen patterns)

### Claude Code skills format
- Research: ClawHub SKILL.md format spec, frontmatter fields, metadata.openclaw schema

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `skills/clawhub/SKILL.md` — Existing ClawHub skill with search/install/remove/list commands
- `seed.md` CronSync section — Complete design doc with YAML spec format, state.json schema, lock file format
- `templates/agent-wrapper.sh.j2` — Shell wrapper template to extend with system prompt

### Established Patterns
- SKILL.md files with YAML frontmatter (name, description, version)
- Jinja2 templates for shell wrapper generation (minijinja)
- include_str! for template embedding in Rust

### Integration Points
- `shell_wrapper.rs` and `agent-wrapper.sh.j2` need update for system prompt file
- Codegen module needs system prompt file generation function
- Default agent's `agent.yaml` or `crons/` directory for CronSync demo specs

</code_context>

<specifics>
## Specific Ideas

- The `/clawhub` skill already has good bones — mainly needs real API endpoints and refined policy gate logic
- CronSync design is fully specified in seed.md — implement as-designed
- System prompt is a small but important piece — ensures CronSync activates without user action

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 04-skills-and-automation*
*Context gathered: 2026-03-22*
