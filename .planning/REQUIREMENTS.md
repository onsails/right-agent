# Requirements: RightClaw

**Defined:** 2026-03-24
**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by native OS-level isolation, orchestrated by a single CLI command.

## v2.1 Requirements

### Permission Model

- [ ] **PERM-01**: Shell wrapper keeps `--dangerously-skip-permissions` (all tools auto-approved including future MCP tools and crons)
- [ ] **PERM-02**: Pre-populate `.claude.json` in agent HOME with bypass-accepted state to suppress warning dialog on every launch
- [ ] **PERM-03**: Permission relay active via Telegram channel as safety net for any edge case prompts that bypass suppression

### HOME Isolation

- [ ] **HOME-01**: Shell wrapper sets `HOME=$AGENT_DIR` before launching claude — agent sees only its own `.claude/`, `.claude.json`, settings, memory
- [ ] **HOME-02**: `rightclaw up` generates per-agent `.claude.json` with workspace trust entries (`hasTrustDialogAccepted`) + bypass-accepted state inside agent dir
- [ ] **HOME-03**: `rightclaw up` symlinks host OAuth credentials (`~/.claude/.credentials.json`) to each agent's `.claude/.credentials.json`
- [ ] **HOME-04**: Shell wrapper forwards git/SSH identity via env vars (`GIT_CONFIG_GLOBAL`, `SSH_AUTH_SOCK`, `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`)
- [ ] **HOME-05**: Generated sandbox `allowWrite` paths use absolute paths (not `~/` which would resolve to agent HOME under override)

### Agent Environment

- [ ] **AENV-01**: `rightclaw up` initializes `.git/` in each agent directory (bare init so CC recognizes trusted workspace)
- [ ] **AENV-02**: `rightclaw up` copies Telegram channel config to agent HOME (`$AGENT_DIR/.claude/channels/telegram/`) when Telegram is configured
- [ ] **AENV-03**: Pre-populated `.claude/` includes: `settings.json` (sandbox config), `settings.local.json` (empty `{}`), `skills/` (copied from init)

### Doctor & Tooling

- [ ] **TOOL-01**: `rightclaw config strict-sandbox` writes `/etc/claude-code/managed-settings.json` with `allowManagedDomainsOnly: true` (opt-in, requires sudo)
- [ ] **TOOL-02**: `rightclaw doctor` warns if `/etc/claude-code/managed-settings.json` exists and may conflict with RightClaw settings

## Future Requirements

### Smart Task Routing (v2.2+)

- **ROUTE-01**: System prompt instructs agent to use background execution for complex tasks with channel feedback
- **ROUTE-02**: Model routing by complexity — opus for hard, sonnet for moderate, haiku subagent for simple questions

## Out of Scope

| Feature | Reason |
|---------|--------|
| Drop --dangerously-skip-permissions | Breaks crons, MCP tools, headless operation. Sandbox is the security layer, not permissions. |
| dontAsk mode as default | Silently denies unknown tools — breaks user-installed MCP servers and crons |
| allowManagedDomainsOnly as default | Machine-wide side effect, needs sudo, conflicts with per-agent domains. Opt-in only. |
| CLAUDE_CONFIG_DIR as primary | Buggy (9+ open issues). HOME override is more reliable and comprehensive. |
| Credential copy instead of symlink | Symlink keeps tokens fresh. CC reads/writes credentials outside sandbox. Race condition risk accepted. |

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| PERM-01 | — | Pending |
| PERM-02 | — | Pending |
| PERM-03 | — | Pending |
| HOME-01 | — | Pending |
| HOME-02 | — | Pending |
| HOME-03 | — | Pending |
| HOME-04 | — | Pending |
| HOME-05 | — | Pending |
| AENV-01 | — | Pending |
| AENV-02 | — | Pending |
| AENV-03 | — | Pending |
| TOOL-01 | — | Pending |
| TOOL-02 | — | Pending |

**Coverage:**
- v2.1 requirements: 13 total
- Mapped to phases: 0
- Unmapped: 13

---
*Requirements defined: 2026-03-24*
*Last updated: 2026-03-24 after initial definition*
