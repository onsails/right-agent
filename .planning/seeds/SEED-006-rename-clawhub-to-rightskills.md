---
id: SEED-006
status: dormant
planted: 2026-03-23
planted_during: v1.0 / manual testing (skill install)
trigger_when: next milestone or skill management phase
scope: Medium
---

# SEED-006: Rename clawhub skill to rightskills, drop policy.yaml mentions, add env/secrets

## Problem

The current `/clawhub` skill (now using skills.sh) has several issues:

1. **Name mismatch** — Skill is called "clawhub" but uses skills.sh (Vercel). Should be renamed to something RightClaw-native like "rightskills" or just "skills"
2. **Policy.yaml references** — The skill's policy gate checks `metadata.openclaw.requires` and references OpenShell policy.yaml. We're migrating to Claude's native sandboxing (SEED-003), so these references are obsolete
3. **No env var support** — Skills that need env vars (like `BROWSER_USE_API_KEY`) have no mechanism to inject them into the agent. Need per-agent env var configuration
4. **No secret management** — Sensitive env vars (API keys) shouldn't be in plain text. Need secretspec or similar pattern

## What happened during testing

Installed `browser-use` skill via Telegram. The skill installed correctly to `.claude/skills/browser-use/` and the policy gate caught 3 issues:
- Binary `browser-use` not installed
- No Chromium/Chrome in sandbox
- Network blocked by default-deny policy

The policy gate works but is tied to OpenShell policy.yaml which we're moving away from.

## Proposed changes

1. **Rename** — `clawhub` → `rightskills` (or `skills`). Update directory, SKILL.md, all references in init.rs
2. **Remove policy gate** — Or rework it to check Claude-native sandbox capabilities instead of OpenShell policy.yaml
3. **Env var injection** — Add `env` section to `agent.yaml`:
   ```yaml
   env:
     BROWSER_USE_API_KEY: "${BROWSER_USE_API_KEY}"  # from host env
     CUSTOM_VAR: "literal-value"
   ```
4. **Secretspec** — Define a `.secrets.yaml` or similar that references secrets from a vault/env without storing them in agent config:
   ```yaml
   secrets:
     - name: BROWSER_USE_API_KEY
       source: env  # or vault, file, etc.
   ```
5. **Shell wrapper update** — Inject env vars into the wrapper script's `exec` command

## Breadcrumbs

- `skills/clawhub/SKILL.md` — current skill file (already rewritten for skills.sh)
- `crates/rightclaw/src/init.rs` — `SKILL_CLAWHUB` const and install path
- `crates/rightclaw/src/agent/types.rs` — AgentConfig (needs `env` field)
- `templates/agent-wrapper.sh.j2` — where env vars would be exported
- SEED-001 (skill-policy audit) — related but focused on OpenShell policy
- SEED-003 (Claude-native sandboxing) — migration away from OpenShell
- SEED-004 (per-agent HOME) — related isolation work

## Scope estimate

Medium — rename is trivial, env var injection is straightforward, secretspec needs design.
