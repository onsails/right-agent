# Phase 13: Policy Gate Rework - Research

**Researched:** 2026-03-26
**Domain:** SKILL.md prose authoring — agent instruction rewrite (no Rust)
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** agentskills.io defines NO structured requirement fields. The `compatibility` prose field is the official channel. `metadata.openclaw.requires.*` is a non-standard OpenClaw extension — must be removed entirely.
- **D-02:** skills.sh uses the same agentskills.io standard. No skills.sh-specific structured extensions exist.
- **D-03:** Gate reads `compatibility` prose field. Claude interprets it (bins, network domains, env vars) — no structured parsing.
- **D-04:** BLOCK conditions (sandbox-enforced): required network domain absent from `sandbox.network.allowedDomains`; required filesystem write not covered by `sandbox.filesystem.allowWrite`.
- **D-05:** WARN only (advisory): missing binaries (`which <bin>`), unset env vars (`printenv <VAR>`). Never block on these.
- **D-06:** Before `npx skills add`, specifically check `skills.sh` and `npmjs.org` are in `allowedDomains`. Warn and suggest `agent.yaml` overrides if missing.
- **D-07:** Update the `/skills` SKILL.md own `compatibility` frontmatter to: `Requires Node.js (npx), internet access to skills.sh and npmjs.org`
- **D-08:** New `/skill-doctor` command added to `skills/skills/SKILL.md` under `## Commands`.
- **D-09:** skill-doctor reads `.claude/skills/installed.json`, iterates installed skills, checks bins/env/network from each skill's `compatibility` field.
- **D-10:** Output format: table with columns skill name, bins status, env vars status, network status, overall status (PASS/WARN/BLOCK).

### Claude's Discretion

- How to parse/identify requirements from the `compatibility` prose field (Claude interprets: "Requires git, docker, internet" → bins: [git, docker], network: internet).
- Exact wording for block/warn messages.
- Whether skill-doctor also checks skills found on disk but not in `installed.json` (recommended: yes, for completeness — matches pattern in `list` command).

### Deferred Ideas (OUT OF SCOPE)

- `rightskills` rename — Phase 14.
- `rightclaw up` frontmatter validation — Phase 15+.
- `rightclaw doctor` skill validation — Phase 15+.
- Structured `metadata.requires.*` frontmatter — only revisit if agentskills.io adds it.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| GATE-01 | `/skills` SKILL.md policy gate drops all references to OpenShell `policy.yaml` and `metadata.openclaw.requires` fields | Current Step 3 (lines 95-122 of SKILL.md) contains all the OpenShell references — replace entirely with compatibility-prose gate |
| GATE-02 | `/skills` SKILL.md policy gate instructs agent to check `settings.json` `allowedDomains` and `allowWrite` against skill requirements before activating | New Step 3 reads `.claude/settings.json` `sandbox.network.allowedDomains` and `sandbox.filesystem.allowWrite` — field paths confirmed in settings.rs |
</phase_requirements>

---

## Summary

Phase 13 is a single-file content rewrite. The only artifact changing is `skills/skills/SKILL.md`. No Rust code, no new files, no tests required.

The current Step 3 (Policy gate audit, lines 95-122) references `metadata.openclaw.requires.bins/env/network/filesystem` — a non-standard extension that was removed from the codebase in Phase 12. It must be replaced with prose-based gate logic that reads the standard `compatibility` field and checks it against the CC-native sandbox configuration in `.claude/settings.json`.

Additionally, a new `### skill-doctor` command section is appended under `## Commands` to provide retroactive compatibility auditing of all installed skills.

**Primary recommendation:** Replace Step 3 wholesale. Read `compatibility` prose, classify requirements into bins/env/network/filesystem, check sandbox config, BLOCK on network/filesystem mismatches, WARN on missing bins/env. Add skill-doctor command. Update frontmatter `compatibility` field.

---

## Standard Stack

No libraries. This phase is pure markdown/prose authoring of a SKILL.md instruction file.

---

## Architecture Patterns

### What Changes and What Stays

```
skills/skills/SKILL.md
├── frontmatter (lines 1-9)        ← Update `compatibility` field (D-07)
├── ## When to Activate            ← UNCHANGED
├── ## Configuration               ← UNCHANGED
├── ## Commands
│   ├── ### search                 ← UNCHANGED
│   ├── ### install <slug>
│   │   ├── Step 1: Preview        ← UNCHANGED
│   │   ├── Step 2: Install        ← UNCHANGED (add D-06 domain check before npx)
│   │   ├── Step 3: Policy gate    ← FULL REPLACEMENT (GATE-01 + GATE-02)
│   │   ├── Step 4: Register       ← UNCHANGED
│   │   └── Step 5: Confirm        ← UNCHANGED
│   ├── ### remove                 ← UNCHANGED
│   ├── ### list                   ← UNCHANGED
│   ├── ### update                 ← UNCHANGED
│   └── ### skill-doctor           ← NEW (D-08 through D-10)
├── ## Error Handling              ← UNCHANGED
└── ## Important Rules             ← Minor update (rule 2 stays, language updated)
```

### New Step 3: Policy Gate Logic

The gate has two distinct tiers:

**Tier 1 — Pre-install domain check (D-06):**
Before running `npx skills add`, check that `skills.sh` and `npmjs.org` are in `allowedDomains`. These are the domains the install command itself needs. `npmjs.org` is in `DEFAULT_ALLOWED_DOMAINS` already; `skills.sh` is NOT — so the warning is meaningful.

**Tier 2 — Skill compatibility check (D-03 through D-05):**
After install, read the downloaded SKILL.md `compatibility` prose. Identify requirements by category:
- **Bins:** any word that looks like a binary name (git, docker, python3, node, jq, etc.)
- **Env vars:** any ALL_CAPS_WITH_UNDERSCORES token that isn't a domain name
- **Network:** any domain-like token (contains dots, e.g. api.openai.com)
- **Filesystem:** any mention of "read-write", "write access", "filesystem write"

BLOCK on: network domain not in `sandbox.network.allowedDomains` OR filesystem write path not in `sandbox.filesystem.allowWrite`.
WARN on: binary not in PATH, env var not set.

### skill-doctor Command Pattern

Follows the same bash-blocks-for-CLI-ops pattern established throughout the file:

```bash
# Read installed skills manifest
cat .claude/skills/installed.json

# For each installed skill — read its compatibility field
head -20 .claude/skills/<name>/SKILL.md

# Check bins
which <bin>

# Check env vars
printenv <VAR>

# Read sandbox config
cat .claude/settings.json
```

Output: single table per D-10. Include skills found on disk (`.claude/skills/*/SKILL.md`) but absent from `installed.json` (labeled source=manual).

### settings.json Field Paths (confirmed from settings.rs)

```json
{
  "sandbox": {
    "filesystem": {
      "allowWrite": ["<agent-path>"],
      "allowRead": ["<agent-path>"]
    },
    "network": {
      "allowedDomains": ["api.anthropic.com", "github.com", "npmjs.org", ...]
    }
  }
}
```

The gate reads `sandbox.network.allowedDomains` and `sandbox.filesystem.allowWrite` — exactly these JSON paths.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead |
|---------|-------------|-------------|
| Structured frontmatter parsing | Custom YAML parser in SKILL.md instructions | Claude's natural language interpretation of `compatibility` prose |
| Domain allow-list management | Auto-expanding settings.json | Require user to update `agent.yaml` and re-run `rightclaw up` |

---

## Common Pitfalls

### Pitfall 1: Leaving Any OpenShell Reference in Step 3
**What goes wrong:** GATE-01 fails. Even a single `metadata.openclaw` or `policy.yaml` mention violates the requirement.
**How to avoid:** Replace Step 3 in its entirety. Do not patch — do a full rewrite of the step.
**Warning signs:** Search for `openclaw`, `policy.yaml`, `metadata.openclaw` in the file after edit.

### Pitfall 2: Blocking on Bins/Env (D-05 Violation)
**What goes wrong:** Gate blocks a skill install because `python3` is missing — but that's a WARN-only condition.
**How to avoid:** BLOCK is only for network domains and filesystem write. Bins and env vars are advisory WARNs.

### Pitfall 3: Forgetting the Pre-Install Domain Check (D-06)
**What goes wrong:** Agent tries `npx skills add` without `skills.sh` in allowedDomains — command fails with no guidance.
**How to avoid:** D-06 check (skills.sh + npmjs.org in allowedDomains) must appear in Step 2, before the `npx skills add` command runs. This is distinct from the post-install skill compatibility check.
**Note:** The warn message wording from CONTEXT.md is: "Warning: `skills.sh` is not in your sandbox `allowedDomains`. Add it to your `agent.yaml` sandbox overrides: `allowed_domains: [skills.sh, registry.npmjs.org]`"

### Pitfall 4: skills.sh vs npmjs.org vs registry.npmjs.org
**What goes wrong:** Using wrong domain in warn message. `npmjs.org` is in DEFAULT_ALLOWED_DOMAINS. The npx install also hits `registry.npmjs.org` (subdomain not covered by `npmjs.org` match depending on CC sandbox implementation).
**Recommendation:** Warn message in D-06 should list both `skills.sh` and `registry.npmjs.org` (CONTEXT.md §Specifics already specifies this exact list).

### Pitfall 5: skill-doctor Ignoring Disk-Only Skills
**What goes wrong:** skill-doctor only reads `installed.json` and misses manually installed skills.
**How to avoid:** Per discretion guidance — scan `.claude/skills/*/SKILL.md` on disk, union with `installed.json` entries. Label untracked as "manual". This matches the existing `list` command behavior (lines 185-186 of current SKILL.md).

---

## Code Examples

### Current Step 3 (to be replaced in full)

From `skills/skills/SKILL.md` lines 95-122:

```markdown
**Step 3: Policy gate audit**

Before activating the skill, audit its permissions. Read the downloaded
`.claude/skills/<name>/SKILL.md` frontmatter and check for `metadata.openclaw` sections.
...
| Network access | `metadata.openclaw.requires.network` | ...
```

Everything from "Step 3" heading through "If all checks pass" must be replaced.

### skill-doctor Output Table (from CONTEXT.md §Specifics)

```markdown
| Skill | Bins | Env Vars | Network | Status |
|-------|------|----------|---------|--------|
| my-skill | git ✓ | API_KEY ✗ | api.example.com ✗ | WARN |
| rightcron | — | — | — | PASS |
```

BLOCK status applies when a network domain is missing from `allowedDomains` or required write path missing from `allowWrite`.

---

## Open Questions

1. **npmjs.org glob matching**
   - What we know: `DEFAULT_ALLOWED_DOMAINS` includes `npmjs.org` (not `registry.npmjs.org`). CC sandbox domain matching behavior (exact vs subdomain wildcard) is not documented in the codebase.
   - What's unclear: Does CC sandbox match `registry.npmjs.org` when `npmjs.org` is in the list?
   - Recommendation: Warn about `registry.npmjs.org` explicitly in D-06 message. User can always add it redundantly — no harm.

2. **`compatibility` field absent from a skill**
   - What we know: `compatibility` is optional in the agentskills.io spec.
   - What's unclear: Should the gate skip, pass, or warn when no `compatibility` field is present?
   - Recommendation: Treat absence as "no requirements" — proceed with install, no gate action needed. State this explicitly in Step 3.

---

## Sources

### Primary (HIGH confidence)
- `skills/skills/SKILL.md` — full current content read directly; lines 95-122 are the target for replacement
- `crates/rightclaw/src/codegen/settings.rs` — confirmed `sandbox.network.allowedDomains`, `sandbox.filesystem.allowWrite` field paths; confirmed `skills.sh` is NOT in `DEFAULT_ALLOWED_DOMAINS`
- `.planning/phases/13-policy-gate-rework/13-CONTEXT.md` — all locked decisions, exact warn message wording, table format
- `.planning/REQUIREMENTS.md` — GATE-01, GATE-02 requirement text

### Secondary (MEDIUM confidence)
- agentskills.io frontmatter spec — researched during /gsd:discuss-phase (2026-03-26), confirmed no structured `requires:` fields

---

## Metadata

**Confidence breakdown:**
- What to change: HIGH — file read directly, exact line ranges identified
- How to implement gate logic: HIGH — all decisions locked in CONTEXT.md
- skill-doctor command: HIGH — output format and data sources fully specified
- npmjs.org subdomain matching: LOW — CC sandbox matching behavior undocumented

**Research date:** 2026-03-26
**Valid until:** Stable — no external dependencies; only internal SKILL.md and settings.rs
