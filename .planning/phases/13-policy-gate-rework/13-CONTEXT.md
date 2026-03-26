# Phase 13: Policy Gate Rework - Context

**Gathered:** 2026-03-26
**Status:** Ready for planning

<domain>
## Phase Boundary

Rewrite the **Step 3 Policy Gate Audit** section in `skills/skills/SKILL.md`:
- Drop all `metadata.openclaw.requires.*` references (non-standard OpenClaw extension)
- Replace with: read the official `compatibility` field, then check against agent's `settings.json`
- Add a new `/skill-doctor` command to the same SKILL.md

This is a SKILL.md content change only. No Rust CLI changes (those are Phase 15+). No rename to `rightskills` (Phase 14).

</domain>

<decisions>
## Implementation Decisions

### agentskills.io Frontmatter Standard
- **D-01:** The agentskills.io specification defines NO structured requirement fields. The official channel for declaring requirements is the `compatibility` prose field (e.g. `compatibility: Requires git, docker, internet access`). `metadata.openclaw.requires.*` was a non-standard OpenClaw extension on the `metadata` field — it must be removed.
- **D-02:** skills.sh uses the same agentskills.io standard. No skills.sh-specific extensions to frontmatter exist. Research confirmed: name, description, license, compatibility (prose), metadata (arbitrary), allowed-tools (experimental) are the only fields.

### Policy Gate Redesign (replaces Step 3)
- **D-03:** Gate reads the `compatibility` prose field from the downloaded skill's SKILL.md. Uses Claude's understanding to identify: required network domains, required binaries, required env vars. No structured parsing needed — prose is the standard.
- **D-04:** **BLOCK** conditions (sandbox-enforced, CC sandbox controls these):
  - Required network domain not present in agent's `.claude/settings.json` `sandbox.network.allowedDomains`
  - Required filesystem write access not covered by `sandbox.filesystem.allowWrite`
- **D-05:** **WARN only** (advisory, not sandbox-enforced):
  - Missing binaries: run `which <bin>` for each binary mentioned in compatibility. Report as warning, not block.
  - Unset env vars: check `printenv <VAR>` for each env var mentioned. Report as warning, not block.
- **D-06:** Before running `npx skills add`, specifically check if `skills.sh` and `npmjs.org` are in `allowedDomains`. Warn and suggest adding them to `agent.yaml` `sandbox.allowed_domains` overrides if missing.
- **D-07:** Update the `/skills` SKILL.md own `compatibility` field to: `Requires Node.js (npx), internet access to skills.sh and npmjs.org`

### skill-doctor Command
- **D-08:** New `/skill-doctor` command added to `skills/skills/SKILL.md` (a new section under `## Commands`).
- **D-09:** skill-doctor reads `.claude/skills/installed.json`, iterates all installed skills, reads each skill's `compatibility` field, and checks:
  1. Bins: `which <bin>` for each mentioned binary
  2. Env vars: `printenv <VAR>` for each mentioned env var
  3. Network: compare mentioned domains against `.claude/settings.json` `allowedDomains`
- **D-10:** Output format is a table: skill name × capability × status (PASS / WARN / BLOCK). One table for the full matrix.

### Claude's Discretion
- How to parse/identify requirements from the `compatibility` prose (Claude interprets: `Requires git, docker, internet` → bins: [git, docker], network: internet).
- Exact wording for block/warn messages.
- Whether skill-doctor also checks skills found on disk but not in installed.json (probably yes, for completeness).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Requirements
- `.planning/REQUIREMENTS.md` §Policy Gate — GATE-01 and GATE-02 (the two requirements this phase addresses)

### Files being modified
- `skills/skills/SKILL.md` — the only file changed in this phase; read it in full before touching anything

### Settings generation (reference for sandbox field names)
- `crates/rightclaw/src/codegen/settings.rs` — defines `DEFAULT_ALLOWED_DOMAINS`, `allowedDomains`, `allowWrite` field names as they appear in generated settings.json

### agentskills.io specification (external, researched 2026-03-26)
- Official fields: `name`, `description`, `license`, `compatibility` (prose), `metadata` (arbitrary), `allowed-tools` (experimental)
- `compatibility` field examples: `"Requires git, docker, jq, and access to the internet"`, `"Requires Python 3.14+ and uv"`
- No structured requirement fields exist — confirmed from spec and ecosystem research

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `skills/skills/SKILL.md` §Step 3 — current policy gate; the entire step gets replaced; surrounding steps (1, 2, 4, 5) stay unchanged
- `crates/rightclaw/src/codegen/settings.rs` `DEFAULT_ALLOWED_DOMAINS` — lists what domains are allowed by default: `api.anthropic.com`, `github.com`, `npmjs.org`, `crates.io`, `agentskills.io`, `api.telegram.org`. Note: `skills.sh` is NOT in defaults — the gate warning is warranted.

### Established Patterns
- Current gate structure: Step 3 is a subsection under `### install <slug>`. `skill-doctor` is a new top-level `### skill-doctor` command under `## Commands`.
- The SKILL.md already uses bash command blocks for all CLI operations — skill-doctor follows the same pattern.

### Integration Points
- `installed.json` at `.claude/skills/installed.json` — tracks installed skills with `slug`, `installed_at`, `path`, `source`. skill-doctor reads this as its manifest.
- `settings.json` at `.claude/settings.json` (relative to agent cwd since agent dir = HOME) — skill-doctor reads `sandbox.network.allowedDomains` and `sandbox.filesystem.allowWrite` from here.

</code_context>

<specifics>
## Specific Ideas

- Gate warn message for missing npx domains: "Warning: `skills.sh` is not in your sandbox `allowedDomains`. Add it to your `agent.yaml` sandbox overrides: `allowed_domains: [skills.sh, registry.npmjs.org]`"
- skill-doctor output table:

  | Skill | Bins | Env Vars | Network | Status |
  |-------|------|----------|---------|--------|
  | my-skill | git ✓ | API_KEY ✗ | api.example.com ✗ | WARN |
  | rightcron | — | — | — | PASS |

</specifics>

<deferred>
## Deferred Ideas

- **`rightskills` rename** — rename `/skills` skill directory and constant to `rightskills`. Phase 14.
- **`rightclaw up` frontmatter validation** — validate installed skill compatibility on agent launch. Phase 15+.
- **`rightclaw doctor` skill validation** — surface compatibility issues in the CLI doctor command. Phase 15+.
- **Structured metadata.requires.*** — the user asked about structured frontmatter; research confirmed the agentskills.io standard uses prose `compatibility` only. If skills.sh ever adds a structured `requires:` extension, revisit.

</deferred>

---

*Phase: 13-policy-gate-rework*
*Context gathered: 2026-03-26*
