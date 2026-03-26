# Requirements: RightClaw v2.2 Skills Registry

**Defined:** 2026-03-25
**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, orchestrated by a single CLI command.

## v2.2 Requirements

### Env Var Injection

- [x] **ENV-01**: User can declare `env:` key-value pairs in `agent.yaml` that get injected into the agent's shell environment before `exec claude`
- [x] **ENV-02**: Injected env var values are properly shell-quoted in the generated wrapper script (no injection, no breakage on spaces/special chars)
- [x] **ENV-03**: Env vars are injected before the `export HOME=` override so identity vars (`GIT_AUTHOR_NAME`, etc.) are not shadowed
- [x] **ENV-04**: `installed.json` is created-if-absent (not overwritten) on every `rightclaw up`, preserving user-installed skill registry state
- [x] **ENV-05**: `agent.yaml` documentation (comments in generated template) warns that `env:` values are stored in plaintext — not for secrets

### Skills Registry

- [x] **SKILLS-01**: `/clawhub` skill and `skills/clawhub/` directory are removed from the codebase; `SKILL_CLAWHUB` constant in Rust is renamed to `SKILL_SKILLS`
- [x] **SKILLS-02**: `rightclaw init` and `rightclaw up` install `/skills` skill (from `skills/skills/SKILL.md`) into each agent's `.claude/skills/` instead of `/clawhub`
- [x] **SKILLS-03**: `/skills` skill uses skills.sh (Vercel) as primary registry: `npx skills find <query>` for search, `npx skills add <owner>/<repo> -a claude` for install
- [x] **SKILLS-04**: `/skills` skill has no ClawHub fallback — ClawHub is removed completely (not deferred, not opt-in)
- [x] **SKILLS-05**: `rightclaw up` removes stale `.claude/skills/clawhub/` directory from existing agent dirs on first run after upgrade

### Policy Gate

- [x] **GATE-01**: `/skills` SKILL.md policy gate drops all references to OpenShell `policy.yaml` and `metadata.openclaw.requires` fields
- [x] **GATE-02**: `/skills` SKILL.md policy gate instructs the agent to check `settings.json` `allowedDomains` and `allowWrite` against skill requirements before activating

### rightskills Rename

- [ ] **RS-01**: Source directory renamed from `skills/skills/` to `skills/rightskills/` via `git mv`
- [ ] **RS-02**: SKILL.md frontmatter `name:` field updated from `skills` to `rightskills`; H1 heading updated from `# /skills` to `# /rightskills`
- [ ] **RS-03**: Rust constant renamed from `SKILL_SKILLS` to `SKILL_RIGHTSKILLS`; `include_str!` path and install path tuple updated to reference `skills/rightskills/SKILL.md`
- [ ] **RS-04**: All test assertions in `skills.rs` and `init.rs` that referenced `.claude/skills/skills/SKILL.md` updated to `.claude/skills/rightskills/SKILL.md`; workspace builds and all tests pass

## Future Requirements (Deferred)

- Secretspec / vault integration for sensitive env vars (`env:` is plaintext only) — v2.3
- ClawHub re-evaluation post-stabilization — only if registry recovers trust post-ClawHavoc
- `/skills` policy gate: programmatic Rust enforcement (currently instruction-based SKILL.md) — v2.3

## Out of Scope

- ClawHub as fallback or opt-in — removed completely per user decision
- `npx` sandbox domain whitelisting in managed-settings — deferred (document approach in SKILL.md instead)
- Skill version pinning or lockfile — v2.3
- Stale `.claude/skills/skills/` cleanup in `cmd_up` — not in production, deferred

## Traceability

| REQ-ID | Phase | Status |
|--------|-------|--------|
| ENV-01 | Phase 11 | Complete |
| ENV-02 | Phase 11 | Complete |
| ENV-03 | Phase 11 | Complete |
| ENV-04 | Phase 11 | Complete |
| ENV-05 | Phase 11 | Complete |
| SKILLS-01 | Phase 12 | Complete |
| SKILLS-02 | Phase 12 | Complete |
| SKILLS-03 | Phase 12 | Complete |
| SKILLS-04 | Phase 12 | Complete |
| SKILLS-05 | Phase 12 | Complete |
| GATE-01 | Phase 13 | Complete |
| GATE-02 | Phase 13 | Complete |
| RS-01 | Phase 14 | Pending |
| RS-02 | Phase 14 | Pending |
| RS-03 | Phase 14 | Pending |
| RS-04 | Phase 14 | Pending |
