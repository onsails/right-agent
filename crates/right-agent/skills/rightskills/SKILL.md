---
name: rightskills
description: >-
  Manages Agent Skills for this Right Agent agent via skills.sh (Vercel's Agent Skills directory).
  Searches the skills.sh registry, installs skills by owner/repo slug, removes installed skills,
  and lists all installed skills. Use when the user wants to find, install, remove, update, or
  list Claude Code skills, or mentions skills.sh, skill packages, or skill management.
version: 0.3.0
compatibility: Requires Node.js (npx), internet access to skills.sh and npmjs.org
---

# /rightskills -- Agent Skills Manager (skills.sh)

You are the skill manager for this Right Agent agent, powered by [skills.sh](https://skills.sh) (Vercel's Agent Skills directory).

## When to Activate

Activate this skill when the user:
- Wants to install a skill (e.g., "install a skill", "add skill", "get me a skill for...")
- Wants to find or search for skills (e.g., "find skills", "search skills", "what skills are available")
- Mentions skills.sh, skill packages, or skill management
- Wants to remove or uninstall a skill (e.g., "remove skill", "uninstall skill")
- Wants to list installed skills (e.g., "list installed skills", "what skills do I have")
- Wants to update skills (e.g., "update skills", "upgrade skill")

## Configuration

**Directory:** [skills.sh](https://skills.sh) -- Vercel's open Agent Skills directory

**CLI:** `npx skills` -- the official CLI for managing skills

**Skill install path:** `.claude/skills/<name>/` (standard Agent Skills path, relative to agent cwd)

**Registry file:** `.claude/skills/installed.json` (tracks all installed skills for this agent)

## Commands

### search \<query\>

Search the skills.sh directory for skills matching a query.

1. Run the search command:
   ```bash
   npx skills find "<query>"
   ```

2. If `npx skills find` is not available or fails, fall back to GitHub topic search:
   ```bash
   gh search repos "<query>" --topic=agent-skill --json fullName,description,stargazersCount --limit 20
   ```

3. Present results as a formatted table:

   | Name | Description | Stars |
   |------|-------------|-------|
   | vercel-labs/agent-skills | Official Vercel skills collection | 1200 |

4. Ask the user if they want to install any of the results.

**Error handling:**
- If `npx` is not available: inform the user to install Node.js. Suggest manual git clone as a fallback: `git clone <repo-url> .claude/skills/<name>/`
- If the search returns no results: suggest broadening the query or browsing https://skills.sh directly.

### install \<slug\>

Install a skill by its slug (e.g., `vercel-labs/agent-skills`). The slug is in `owner/repo` format (GitHub shorthand).

**Step 1: Preview available skills**

```bash
npx skills add <slug> --list
```

This lists all skills in the repository without installing. Let the user pick which ones to install if the repo contains multiple skills.

**Step 2: Install the skill**

```bash
npx skills add <slug> -a claude --copy -y
```

Flags:
- `-a claude` -- target Claude Code agent
- `--copy` -- copy files instead of symlinking (ensures portability)
- `-y` -- skip confirmation prompts

If `npx` is unavailable, fall back to manual installation:
```bash
git clone --depth 1 "https://github.com/<slug>.git" /tmp/skills-install
# Copy SKILL.md (and any accompanying files) to .claude/skills/<name>/
mkdir -p .claude/skills/<name>
cp -r /tmp/skills-install/skills/<name>/* .claude/skills/<name>/ 2>/dev/null || cp -r /tmp/skills-install/* .claude/skills/<name>/
rm -rf /tmp/skills-install
```

**Pre-install domain check**

Before running `npx skills add`, verify that the install command itself can reach its required domains.

Read `.claude/settings.json` and check `sandbox.network.allowedDomains`:

```bash
cat .claude/settings.json
```

- If `skills.sh` is absent from `allowedDomains`: warn and stop.
  > Warning: `skills.sh` is not in your sandbox `allowedDomains`. Add it to your `agent.yaml` sandbox overrides: `allowed_domains: [skills.sh, registry.npmjs.org]`
  > After updating `agent.yaml`, run `right up` to regenerate `settings.json`, then retry.
- If `registry.npmjs.org` (or `npmjs.org`) is absent: add the same warning.
- If both are present: proceed with `npx skills add`.

**Step 3: Policy gate audit**

After the skill files are downloaded to `.claude/skills/<name>/`, read the skill's SKILL.md and check its `compatibility` field:

```bash
head -20 .claude/skills/<name>/SKILL.md
```

**If the skill has no `compatibility` field:** treat as no requirements — proceed directly to Step 4.

**If a `compatibility` field is present:** interpret it as natural language to identify requirement categories:

- **Bins:** binary names (e.g., `git`, `docker`, `python3`, `node`, `jq`) — any named executable tool
- **Env vars:** ALL_CAPS_WITH_UNDERSCORES tokens that are not domain names (e.g., `OPENAI_API_KEY`, `GITHUB_TOKEN`)
- **Network:** domain-like tokens containing dots (e.g., `api.openai.com`, `registry.npmjs.org`)
- **Filesystem write:** any phrase like "read-write access", "writes to", "modifies files outside", "filesystem write"

Run the following checks:

```bash
# Read the agent's sandbox configuration
cat .claude/settings.json

# Check each required binary
which <bin>

# Check each required env var
printenv <VAR>
```

Apply the two-tier gate:

**BLOCK (sandbox-enforced — do not proceed):**
- A required network domain is NOT present in `sandbox.network.allowedDomains` in `.claude/settings.json`
- A required filesystem write path is NOT covered by `sandbox.filesystem.allowWrite` in `.claude/settings.json`

**WARN only (advisory — proceed after informing the user):**
- A required binary is not found in PATH (`which` returns nothing)
- A required env var is not set (`printenv` returns nothing)

Display the full audit result as a table before deciding to proceed or block:

| Permission | Required | Status |
|------------|----------|--------|
| Binary: git | Yes | OK |
| Env: OPENAI_API_KEY | Yes | WARN — not set |
| Network: api.openai.com | Yes | BLOCK — not in allowedDomains |
| Filesystem: read-write | Yes | OK |

**If any BLOCK condition is present:**
> Installation blocked. The skill requires network or filesystem access that your agent's sandbox does not allow. Add the missing domains or write paths to your `agent.yaml` sandbox section, then run `right up` to regenerate `settings.json`, and retry the installation.

Do NOT auto-expand the agent's sandbox config. The user must explicitly update `agent.yaml`.

**If no BLOCK conditions (WARNs only or all pass):** inform the user of any warnings, then proceed to Step 4.

**Step 4: Register in installed.json**

Read `.claude/skills/installed.json`. If it does not exist, create it with `{}`.

Add an entry for the new skill:

```json
{
  "<name>": {
    "slug": "vercel-labs/agent-skills",
    "installed_at": "2026-03-22T10:00:00Z",
    "path": ".claude/skills/<name>",
    "source": "skills.sh"
  }
}
```

All timestamps MUST use UTC ISO 8601 format with the `Z` suffix. Generate the timestamp with:
```bash
date -u +"%Y-%m-%dT%H:%M:%SZ"
```

Write the updated JSON back to `.claude/skills/installed.json`.

**Step 5: Confirm installation**

Report to the user:
> Installed **<name>** from skills.sh (`<slug>`). The skill is now available at `.claude/skills/<name>/`.

### remove \<name\>

Remove an installed skill by name.

1. First try the CLI:
   ```bash
   npx skills remove <name> -a claude -y
   ```

2. If the CLI is unavailable, remove manually:
   - Read `.claude/skills/installed.json`. If it does not exist, inform the user that no skills are tracked.
   - Check if `<name>` exists in the registry.
     - If not found: inform the user that the skill is not installed (or not tracked). Check if `.claude/skills/<name>/` exists on disk -- if so, suggest it may have been manually installed.
   - If found:
     - Delete the `.claude/skills/<name>/` directory: `rm -rf .claude/skills/<name>/`
     - Remove the entry from `.claude/skills/installed.json`
     - Write the updated JSON back to `.claude/skills/installed.json`

3. Confirm removal:
   > Removed **<name>** and unregistered it from installed.json.

### list

List all installed skills for this agent.

1. First try the CLI:
   ```bash
   npx skills list -a claude
   ```

2. Additionally, read `.claude/skills/installed.json`. If it does not exist, start with an empty registry.

3. Scan the `.claude/skills/` directory for subdirectories containing a `SKILL.md` file. Include any skills found on disk but not in `installed.json` (these are manually installed or installed via `npx skills add`).

4. Present a table:

   | Name | Source | Installed |
   |------|--------|-----------|
   | agent-browser | skills.sh | 2026-03-22T10:00:00Z |
   | my-custom-skill | manual | - |

   - Source is `skills.sh` if tracked in `installed.json`, `manual` if found on disk only.
   - Installed date comes from `installed.json`; show `-` for manually installed skills.

### update

Update installed skills to their latest versions.

```bash
npx skills update -a claude
```

If `npx` is unavailable, suggest the user re-install the skill: `npx skills add <slug> -a claude --copy -y`

### skill-doctor

Audit all installed skills against this agent's current sandbox configuration. Reports whether each skill's requirements are satisfied.

**Step 1: Discover installed skills**

Union two sources to build the complete skill list:

```bash
# Source 1: tracked skills
cat .claude/skills/installed.json

# Source 2: scan disk for any SKILL.md not in installed.json
ls .claude/skills/
```

Skills found on disk but absent from `installed.json` are labeled `manual` (matching the `list` command convention).

**Step 2: Read sandbox configuration**

```bash
cat .claude/settings.json
```

Extract `sandbox.network.allowedDomains` and `sandbox.filesystem.allowWrite` from the output.

**Step 3: Check each skill**

For each skill (from both sources):

```bash
# Read the skill's compatibility field
head -20 .claude/skills/<name>/SKILL.md
```

If the skill has no `compatibility` field: mark all checks as `—` (no requirements) and status as `PASS`.

If a `compatibility` field is present: interpret it to identify bins, env vars, network domains, and filesystem requirements (same logic as the install gate in Step 3 of `install`).

```bash
# Check each required binary
which <bin>

# Check each required env var
printenv <VAR>
```

Compare required network domains against `allowedDomains` and required filesystem write paths against `allowWrite`.

**Step 4: Report results**

Present a single table covering all skills:

| Skill | Source | Bins | Env Vars | Network | Status |
|-------|--------|------|----------|---------|--------|
| rightcron | skills.sh | — | — | — | PASS |
| my-skill | skills.sh | git ✓ | API_KEY ✗ | api.example.com ✗ | WARN |
| custom-tool | manual | docker ✓ | — | api.custom.com ✗ | BLOCK |

Status rules:
- **PASS**: all checks pass (or no requirements)
- **WARN**: one or more required binaries missing from PATH or env vars unset — but no sandbox violations
- **BLOCK**: one or more required network domains absent from `allowedDomains`, or required write path absent from `allowWrite`

After the table, summarize any actionable items:
- For each BLOCK: name the missing domain or write path and remind the user to update `agent.yaml` sandbox overrides and run `right up`.
- For each WARN: name the missing binary or env var and suggest installing/setting it.

## Error Handling

- **npx not available:** Inform the user to install Node.js (v18+). Suggest manual git clone as fallback.
- **Network errors:** If the GitHub API or npx commands fail, inform the user and suggest manual git clone: `git clone <repo-url> .claude/skills/<name>/`
- **Missing installed.json:** Create an empty `{}` file automatically -- this is normal for a fresh agent.
- **Skills not found:** Suggest browsing https://skills.sh or searching GitHub directly with `gh search repos`.

## Important Rules

1. All timestamps MUST use UTC ISO 8601 format with the `Z` suffix (e.g., `2026-03-22T10:00:00Z`).
2. Never install a skill without running the pre-install domain check (Step 2) and the policy gate audit (Step 3) first. No exceptions.
3. Never auto-expand the agent's sandbox config to accommodate a skill's requirements. The user must explicitly update `agent.yaml` sandbox overrides.
4. Each agent has its own `.claude/skills/` directory and `installed.json` -- no shared or global skill location.
5. Skills installed manually (via git clone or `npx skills add`) appear in `list` output but are tracked differently.
6. The `--copy` flag is mandatory for `npx skills add` to ensure files are portable and not symlinked.
