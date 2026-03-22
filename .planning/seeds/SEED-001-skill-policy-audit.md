---
id: SEED-001
status: dormant
planted: 2026-03-22
planted_during: v1.0 / Phase 04 (skills-and-automation)
trigger_when: next milestone or dedicated security/skill-management phase
scope: Large
---

# SEED-001: Skill-policy compatibility audit with env var management

Audit installed skills against agent sandbox policy. Parse SKILL.md frontmatter to extract required binaries, network access, env vars, and filesystem paths. Flag incompatibilities before runtime. Suggest automatic policy adjustments. Introduce per-agent env var support and secretspec for sensitive values.

## Why This Matters

Three reinforcing reasons:

1. **Security gap** — Without pre-install auditing, a skill could require binaries, network endpoints, or env vars that the agent's OpenShell sandbox blocks. This causes silent runtime failures that are hard to diagnose. The user installs a skill, it looks installed, but it can't actually work.

2. **UX improvement** — Users shouldn't manually cross-reference SKILL.md frontmatter with their agent's policy.yaml. RightClaw should do this automatically and suggest exact policy changes needed.

3. **Ecosystem trust** — RightClaw's core pitch is "security-first alternative to OpenClaw." If we can't validate skill compatibility before activation, we're not delivering on that promise. This is table-stakes for the brand.

## When to Surface

**Trigger:** Next milestone planning — this needs dedicated brainstorming on architecture before implementation.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- Skills management, ClawHub integration, or skill lifecycle work
- Security hardening or policy management improvements
- Agent configuration or environment management
- Per-agent customization features

## Scope Estimate

**Large** — This is a full milestone effort with multiple interconnected concerns:

1. **SKILL.md frontmatter parser** — Parse `metadata.openshell` and other frontmatter fields to extract binary deps, network requirements, env vars, filesystem needs
2. **Policy compatibility checker** — Compare extracted requirements against agent's policy.yaml (filesystem_policy, network_policies, allowed binaries)
3. **Policy suggestion engine** — Generate exact YAML diffs to make policy compatible with a skill
4. **Per-agent env vars** — Currently not supported. Need to decide: plain env vars in agent.yaml? Encrypted? Both?
5. **Secretspec** — Define a spec for declaring required secrets (API keys, tokens) that skills need. Consider: where stored, how injected, rotation, agent isolation
6. **Interactive audit UX** — `rightclaw audit <agent>` or integrated into `/clawhub install` flow

## Breadcrumbs

Related code and decisions found in the current codebase:

- `skills/clawhub/SKILL.md` — Already has policy gate concept at step 4 of install flow (lines 32-38). Current implementation is instruction-based, not enforced by Rust code.
- `templates/right/policy.yaml` — Production OpenShell policy with filesystem_policy, network_policies sections. This is what skills would be audited against.
- `crates/rightclaw/src/agent/types.rs` — Agent config types. Would need env var fields.
- `crates/rightclaw/src/agent/discovery.rs` — Agent discovery logic. Would need to discover installed skills per agent.
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — Shell wrapper generation. Would need to inject env vars into agent processes.
- `.planning/phases/04-skills-and-automation/04-01-PLAN.md` — Phase 4 plan mentions "policy gate blocks installation" as a must-have truth, but current implementation delegates to Claude instructions rather than Rust enforcement.
- `PROJECT.md` line 52 — "Policy gate for installed skills — audit permissions before activation" is an active requirement.

## Notes

- Phase 4 is currently in progress with skill management as instruction-based SKILL.md files. The policy gate there is "Claude reads frontmatter and warns the user" — not programmatic enforcement. This seed is about making it real in Rust.
- Per-agent env vars don't exist yet. The shell wrapper (`shell_wrapper.rs`) generates scripts that launch `openshell sandbox create`, but there's no mechanism to inject env vars into the sandbox.
- Consider whether secretspec should be a RightClaw-specific concept or propose it as an OpenClaw/ClawHub ecosystem standard. If we define it well, ClawHub skill authors could declare their secret requirements in SKILL.md frontmatter.
- OpenShell's `openshell sandbox create` likely supports env var passthrough — needs investigation during implementation.
- This needs serious brainstorming before planning. Multiple design decisions with ecosystem-wide implications.
