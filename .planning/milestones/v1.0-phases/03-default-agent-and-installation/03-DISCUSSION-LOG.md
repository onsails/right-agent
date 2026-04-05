# Phase 3: Default Agent and Installation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-22
**Phase:** 3-Default Agent and Installation
**Areas discussed:** Onboarding flow, OpenShell policy, Install script, Telegram setup

---

## Onboarding Flow

### Question format

| Option | Description | Selected |
|--------|-------------|----------|
| Match OpenClaw | Same 4 questions: name, creature type, vibe, emoji | ✓ |
| Simplified | Just name and vibe | |
| Custom | User-specified flow | |

**User's choice:** Match OpenClaw — drop-in compatible

### Telegram in onboarding

| Option | Description | Selected |
|--------|-------------|----------|
| Part of onboarding | BOOTSTRAP.md asks about Telegram at end | ✓ (initially) |
| Separate step | Manual or separate skill | |
| You decide | Claude's discretion | |

**User's choice:** Initially selected "Part of onboarding" but later corrected — MCP servers can't be loaded mid-session, so Telegram setup must happen in `rightclaw init` BEFORE agent launches. BOOTSTRAP.md only does identity questions.

---

## OpenShell Policy

### Filesystem access

| Option | Description | Selected |
|--------|-------------|----------|
| Home dir read-write | ~/.rightclaw/ and user home r/w | |
| Agent dir only | Agent dir r/w, everything else r/o | ✓ |
| Broad access | R/W broadly for project work | |

**User's choice:** Agent dir only — start with least privileges, expand if needed

### Network access

| Option | Description | Selected |
|--------|-------------|----------|
| GitHub + Telegram | api.github.com, api.telegram.org | ✓ |
| Allowlist common | + npm, pypi, crates.io | |
| Open outbound | Allow all outbound | |

**User's choice:** GitHub + Telegram only

### Additional requirement (user-initiated)
**User requested:** Policy.yaml must include comprehensive comments showing how to:
- Allow all outbound hosts
- Add read/read-write access to specific directories
- Give broad filesystem access
- Add network endpoints

This makes the policy file a self-documenting reference.

---

## Install Script

### Binary delivery

| Option | Description | Selected |
|--------|-------------|----------|
| Pre-built releases | GitHub Releases binaries | ✓ |
| Cargo install | From crates.io | |
| Build from source | Clone + cargo build | |

**User's choice:** Pre-built releases

### Dependency installation

| Option | Description | Selected |
|--------|-------------|----------|
| Each tool's installer | Call official install scripts | |
| Check and skip | Only install missing ones | ✓ |
| You decide | Claude's discretion | |

**User's choice:** Check and skip — respect existing installations

---

## Telegram Setup

### MCP config creation

| Option | Description | Selected |
|--------|-------------|----------|
| Bootstrap creates it | BOOTSTRAP.md asks for token | |
| Pre-configured template | init ships placeholder | |
| Both | Template + bootstrap fills in | |

**User's choice:** Neither — user clarified MCP must be configured BEFORE agent launch (CC can't reload MCP mid-session). So `rightclaw init` handles token prompt.

### Token input method

| Option | Description | Selected |
|--------|-------------|----------|
| Interactive prompt | Terminal prompt during init | |
| Flag-based | --telegram-token flag | |
| Both | Flag priority, interactive fallback | ✓ |

**User's choice:** Both — flag takes priority, interactive prompt as fallback

---

## Claude's Discretion

- BOOTSTRAP.md conversation design
- doctor command output formatting
- install.sh error handling
- .mcp.json exact structure

## Deferred Ideas

None
