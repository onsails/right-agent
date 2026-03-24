# Phase 6: Sandbox Configuration - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-24
**Phase:** 06-sandbox-configuration
**Areas discussed:** Default sandbox settings, agent.yaml override format, --no-sandbox wiring, Settings lifecycle

---

## Default Sandbox Settings — Network

| Option | Description | Selected |
|--------|-------------|----------|
| Minimal + agent-specific | api.anthropic.com always. Telegram/skills.sh only if configured. Nothing else. | |
| Generous defaults | Common dev domains: github.com, npmjs.org, crates.io, api.anthropic.com, agentskills.io, api.telegram.org | ✓ |
| No network restrictions | Don't set allowedDomains. Let CC prompt for each domain. | |

**User's choice:** Generous defaults
**Notes:** Includes common development infrastructure. User adds more via agent.yaml.

## Default Sandbox Settings — Filesystem

| Option | Description | Selected |
|--------|-------------|----------|
| Agent dir only | allowWrite scoped to agent's directory only | ✓ |
| Agent dir + /tmp | Agent dir + system temp for intermediate files | |
| Agent dir + /tmp + home config | Agent dir + /tmp + ~/.config, ~/.cache for tools | |

**User's choice:** Agent dir only
**Notes:** Strictest option. Agents can only write to their own directory.

---

## agent.yaml Override Format

| Option | Description | Selected |
|--------|-------------|----------|
| Nested sandbox section | sandbox:\n  allow_write: [...]\n  allowed_domains: [...] | ✓ |
| Flat prefixed fields | sandbox_allow_write: [...] | |

**User's choice:** Nested sandbox section
**Notes:** User asked about `//` prefix convention — explained it's CC's convention for absolute paths in sandbox settings.

---

## --no-sandbox Wiring

| Option | Description | Selected |
|--------|-------------|----------|
| Generate with sandbox.enabled: false | Still creates settings.json but sandbox disabled. Other settings still apply. | ✓ |
| Skip sandbox section entirely | Omit sandbox key from settings.json. | |
| Skip settings.json generation | Don't generate settings.json at all. | |

**User's choice:** Generate with sandbox.enabled: false
**Notes:** Non-sandbox settings (skipDangerousModePermissionPrompt, etc.) still needed.

---

## Settings Lifecycle

| Option | Description | Selected |
|--------|-------------|----------|
| Overwrite on every `up` | Deterministic. All customization through agent.yaml. | ✓ |
| Generate if missing, skip if exists | Preserves user edits but risks drift. | |
| Merge with existing | Most complex but most flexible. | |

**User's choice:** Overwrite on every `up`
**Notes:** agent.yaml is single source of truth. No manual editing of .claude/settings.json.

---

## Claude's Discretion

- Exact JSON key naming (match CC schema)
- Test strategy for settings generation
- Whether to add denyRead/denyWrite defaults
- Path prefix resolution strategy

## Deferred Ideas

None — discussion stayed within phase scope
