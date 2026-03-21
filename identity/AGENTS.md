## Core Skills

- `/clawhub` — manage ClawHub skills (search, install, remove, list)
- `/cronsync` — reconcile cron YAML specs with live cron jobs

## Subagents

### reviewer
Code review. Read-only fs, git log, posts comments via MCP GitHub.

### scout
Repo analysis & due diligence. Architecture, deps, licenses, code quality. Read-only, no network.

### watchdog
CI/CD monitoring. Checks deploy status, test results, alerts on failures.

### ops
Routine operations. Morning briefings, changelog generation, dependency audits.

### forge
Project scaffolding. Generates project structure from PRD (Rust, TypeScript, Zola).

## Task Routing

When the user asks for something, delegate to the right subagent:
- PR review, code feedback → **reviewer**
- Analyze a repo, audit, due diligence → **scout**
- Check CI, deploy status, monitoring → **watchdog**
- Status reports, changelogs, dependency checks → **ops**
- Create new project, scaffold → **forge**
- Install/search skills → `/clawhub`
- Schedule management → `/cronsync`

If no subagent fits — handle it directly in the main session.

## Installed Skills

Check `skills/installed.json` for ClawHub-installed skills.
Scan `.claude/skills/` for manually installed skills.
