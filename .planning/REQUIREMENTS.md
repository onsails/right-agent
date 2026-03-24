# Requirements: RightClaw

**Defined:** 2026-03-24
**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, orchestrated by a single CLI command.

## v2.0 Requirements

### Sandbox Migration

- [x] **SBMG-01**: User can run `rightclaw up` without OpenShell installed — CC native sandbox used instead
- [x] **SBMG-02**: All OpenShell code paths removed from codebase (sandbox.rs, openshell dep checks, sandbox create/destroy lifecycle)
- [x] **SBMG-03**: Shell wrapper launches `claude` directly instead of wrapping with `openshell sandbox create`
- [x] **SBMG-04**: `rightclaw down` no longer attempts OpenShell sandbox destroy — process-compose shutdown is sufficient
- [x] **SBMG-05**: OpenShell policy.yaml files removed from default agent template

### Sandbox Configuration

- [x] **SBCF-01**: `rightclaw up` generates per-agent `.claude/settings.json` in each agent directory with sandbox enabled
- [x] **SBCF-02**: Generated settings.json includes filesystem restrictions (allowWrite scoped to agent dir + workspace)
- [x] **SBCF-03**: Generated settings.json includes network restrictions (allowedDomains for Telegram, skills.sh, Anthropic API)
- [x] **SBCF-04**: Generated settings.json sets `allowUnsandboxedCommands: false` and `autoAllowBashIfSandboxed: true` as secure defaults
- [x] **SBCF-05**: User can override sandbox settings per-agent via `agent.yaml` sandbox section (allowWrite, allowedDomains, excludedCommands)
- [x] **SBCF-06**: agent.yaml sandbox overrides merge with (not replace) generated defaults

### Platform Compatibility

- [ ] **PLAT-01**: `rightclaw doctor` checks for bubblewrap and socat on Linux (not required on macOS)
- [ ] **PLAT-02**: `rightclaw doctor` detects Ubuntu 24.04+ AppArmor restriction on unprivileged user namespaces and provides fix guidance
- [ ] **PLAT-03**: `rightclaw doctor` no longer checks for OpenShell installation
- [ ] **PLAT-04**: `install.sh` installs bubblewrap and socat on Linux (apt/dnf/pacman detection), skips on macOS
- [ ] **PLAT-05**: `install.sh` no longer installs OpenShell

## Future Requirements

### HOME Isolation (v2.1)

- **HOME-01**: Each agent runs with `HOME=$AGENT_DIR` for full settings/memory isolation
- **HOME-02**: Per-agent `.claude.json` trust file generated inside agent dir
- **HOME-03**: Git/SSH identity forwarded via env vars (GIT_CONFIG_GLOBAL, SSH_AUTH_SOCK)
- **HOME-04**: Telegram channel paths work under HOME override

### Shared Memory (v3+)

- **SHMM-01**: MCP memory server (SQLite or knowledge graph) for inter-agent memory sharing
- **SHMM-02**: Tagged memory entries with source agent identification

### Advanced Features (v3+)

- **ADVN-01**: Policy merging when agent has base sandbox config and installed skills with their own requirements

## Out of Scope

| Feature | Reason |
|---------|--------|
| OpenShell integration | Replaced by CC native sandboxing in v2.0 |
| HOME override (this milestone) | Edge cases (trust file, git/SSH, Telegram, credentials) deferred to v2.1 |
| Web UI / dashboard | TUI via process-compose is sufficient |
| Central orchestrator / master agent | Agents are autonomous |
| Agent-to-agent communication | Agents are autonomous |
| Built-in secrets management | Use system-level secrets (env vars, vault) |
| Write/Edit tool sandboxing | CC limitation — only Bash commands sandboxed under bypassPermissions |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| SBMG-01 | Phase 5 | Complete |
| SBMG-02 | Phase 5 | Complete |
| SBMG-03 | Phase 5 | Complete |
| SBMG-04 | Phase 5 | Complete |
| SBMG-05 | Phase 5 | Complete |
| SBCF-01 | Phase 6 | Complete |
| SBCF-02 | Phase 6 | Complete |
| SBCF-03 | Phase 6 | Complete |
| SBCF-04 | Phase 6 | Complete |
| SBCF-05 | Phase 6 | Complete |
| SBCF-06 | Phase 6 | Complete |
| PLAT-01 | Phase 7 | Pending |
| PLAT-02 | Phase 7 | Pending |
| PLAT-03 | Phase 7 | Pending |
| PLAT-04 | Phase 7 | Pending |
| PLAT-05 | Phase 7 | Pending |

**Coverage:**
- v2.0 requirements: 16 total
- Mapped to phases: 16
- Unmapped: 0

---
*Requirements defined: 2026-03-24*
*Last updated: 2026-03-24 after roadmap creation*
