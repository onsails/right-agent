---
id: SEED-008
status: dormant
planted: 2026-03-24
planted_during: v2.0 UAT
trigger_when: next milestone
scope: Small
---

# SEED-008: Managed settings for strict sandbox (silent domain blocking)

## Why This Matters

CC native sandbox prompts users via TUI dialog when agents access non-allowed domains. For headless/Telegram agents, nobody's there to click — the request hangs for 5 minutes until curl times out. The fix is `allowManagedDomainsOnly: true` which silently blocks non-allowed domains, but this setting is **managed-settings only** (cannot go in project-level `.claude/settings.json`).

File-based managed settings work for any CC install (no enterprise required):
- Linux: `/etc/claude-code/managed-settings.json`
- macOS: `/Library/Application Support/ClaudeCode/managed-settings.json`

Caveat: affects ALL CC instances on the machine, not just RightClaw agents.

## When to Surface

**Trigger:** Next milestone — discovered during v2.0 UAT when testing sandbox network enforcement.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- Sandbox or security improvements
- Agent autonomy / headless operation
- Telegram / channel UX improvements

## Scope Estimate

**Small** — `rightclaw init --strict-sandbox` writes managed-settings.json with `allowManagedDomainsOnly: true`. Needs sudo detection, platform-aware path, and clear UX messaging about machine-wide impact.

## Breadcrumbs

- `crates/rightclaw/src/codegen/settings.rs` — current settings generation (project-level)
- `crates/rightclaw/src/init.rs` — init command, where --strict-sandbox flag would go
- CC docs: `/etc/claude-code/managed-settings.json` format
- UAT test 12: curl to non-allowed domain prompted TUI dialog instead of blocking

## Notes

- Managed settings have highest priority — can't be overridden by user/project settings
- `allowedDomains` in managed settings would override our project-level allowedDomains
- Need to decide: should managed settings ONLY set `allowManagedDomainsOnly`, or also move `allowedDomains` there?
- If `allowedDomains` stays project-level but `allowManagedDomainsOnly` is managed, the project domains would be ignored (per docs: "Only allowedDomains from managed settings are respected")
- Possible approach: managed sets `allowManagedDomainsOnly: true` + a superset of common domains. Project-level is ignored for domains. Agent.yaml overrides would need managed-settings path too.
- Alternative: wait for CC to add project-level `allowManagedDomainsOnly` support
