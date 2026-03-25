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

- [ ] **SKILLS-01**: `/clawhub` skill and `skills/clawhub/` directory are removed from the codebase; `SKILL_CLAWHUB` constant in Rust is renamed to `SKILL_SKILLS`
- [ ] **SKILLS-02**: `rightclaw init` and `rightclaw up` install `/skills` skill (from `skills/skills/SKILL.md`) into each agent's `.claude/skills/` instead of `/clawhub`
- [ ] **SKILLS-03**: `/skills` skill uses skills.sh (Vercel) as primary registry: `npx skills find <query>` for search, `npx skills add <owner>/<repo> -a claude` for install
- [ ] **SKILLS-04**: `/skills` skill has no ClawHub fallback — ClawHub is removed completely (not deferred, not opt-in)
- [ ] **SKILLS-05**: `rightclaw up` removes stale `.claude/skills/clawhub/` directory from existing agent dirs on first run after upgrade

### Policy Gate

- [ ] **GATE-01**: `/skills` SKILL.md policy gate drops all references to OpenShell `policy.yaml` and `metadata.openclaw.requires` fields
- [ ] **GATE-02**: `/skills` SKILL.md policy gate instructs the agent to check `settings.json` `allowedDomains` and `allowWrite` against skill requirements before activating

## Future Requirements (Deferred)

- Secretspec / vault integration for sensitive env vars (`env:` is plaintext only) — v2.3
- ClawHub re-evaluation post-stabilization — only if registry recovers trust post-ClawHavoc
- `/skills` policy gate: programmatic Rust enforcement (currently instruction-based SKILL.md) — v2.3

## Out of Scope

- ClawHub as fallback or opt-in — removed completely per user decision
- `npx` sandbox domain whitelisting in managed-settings — deferred (document approach in SKILL.md instead)
- Skill version pinning or lockfile — v2.3

## Traceability

| REQ-ID | Phase | Status |
|--------|-------|--------|
| ENV-01 | Phase 11 | Complete |
| ENV-02 | Phase 11 | Complete |
| ENV-03 | Phase 11 | Complete |
| ENV-04 | Phase 11 | Complete |
| ENV-05 | Phase 11 | Complete |
| SKILLS-01 | Phase 12 | Pending |
| SKILLS-02 | Phase 12 | Pending |
| SKILLS-03 | Phase 12 | Pending |
| SKILLS-04 | Phase 12 | Pending |
| SKILLS-05 | Phase 12 | Pending |
| GATE-01 | Phase 13 | Pending |
| GATE-02 | Phase 13 | Pending |
