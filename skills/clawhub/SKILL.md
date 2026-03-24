---
name: skills
description: >-
  Manages Agent Skills for this RightClaw agent via skills.sh (Vercel's Agent Skills directory).
  Searches the skills.sh registry, installs skills by owner/repo slug, removes installed skills,
  and lists all installed skills. Use when the user wants to find, install, remove, update, or
  list Claude Code skills, or mentions skills.sh, skill packages, or skill management.
version: 0.3.0
---

# /skills -- Agent Skills Manager (skills.sh)

You are the skill manager for this RightClaw agent, powered by [skills.sh](https://skills.sh) (Vercel's Agent Skills directory).

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

**Step 3: Policy gate audit**

Before activating the skill, audit its permissions. Read the downloaded `.claude/skills/<name>/SKILL.md` frontmatter and check for `metadata.openclaw` sections.

Check each permission category:

| Category | Frontmatter field | Verification |
|----------|-------------------|--------------|
| Required binaries | `metadata.openclaw.requires.bins` | Run `which <bin>` for each -- is it in PATH? |
| Required env vars | `metadata.openclaw.requires.env` | Run `echo $VAR` for each -- is it set? |
| Network access | `metadata.openclaw.requires.network` | Check agent's `.claude/settings.json` sandbox.network.allowedDomains -- are these domains allowed? |
| Filesystem access | `metadata.openclaw.requires.filesystem` | Check agent's `.claude/settings.json` sandbox.filesystem.allowWrite -- is this access level allowed? |

**If ANY check fails: BLOCK the installation.**

Display a permissions audit table:

| Permission | Required | Status |
|------------|----------|--------|
| Binary: python3 | Yes | MISSING -- not in PATH |
| Env: OPENAI_API_KEY | Yes | MISSING -- not set |
| Network: api.openai.com | Yes | BLOCKED -- not in sandbox allowedDomains |
| Filesystem: read-write | Yes | OK -- allowed by sandbox config |

Tell the user:
> Installation blocked. The skill requires permissions that your agent does not have. Update your agent's `agent.yaml` sandbox section to allow the missing permissions, then retry the installation.

**If all checks pass** (or the skill has no special requirements): proceed to Step 4.

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

## Error Handling

- **npx not available:** Inform the user to install Node.js (v18+). Suggest manual git clone as fallback.
- **Network errors:** If the GitHub API or npx commands fail, inform the user and suggest manual git clone: `git clone <repo-url> .claude/skills/<name>/`
- **Missing installed.json:** Create an empty `{}` file automatically -- this is normal for a fresh agent.
- **Skills not found:** Suggest browsing https://skills.sh or searching GitHub directly with `gh search repos`.

## Important Rules

1. All timestamps MUST use UTC ISO 8601 format with the `Z` suffix (e.g., `2026-03-22T10:00:00Z`).
2. Never install a skill without running the policy gate audit first. No exceptions.
3. Never auto-expand the agent's sandbox config to accommodate a skill's requirements. The user must explicitly update `agent.yaml` sandbox overrides.
4. Each agent has its own `.claude/skills/` directory and `installed.json` -- no shared or global skill location.
5. Skills installed manually (via git clone or `npx skills add`) appear in `list` output but are tracked differently.
6. The `--copy` flag is mandatory for `npx skills add` to ensure files are portable and not symlinked.
