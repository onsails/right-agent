---
name: clawhub
description: >-
  Manages ClawHub skills for this RightClaw agent. Searches the ClawHub registry,
  installs skills by slug, removes installed skills, and lists all installed skills.
  Use when the user wants to find, install, remove, update, or list Claude Code skills,
  or mentions ClawHub, skill packages, or skill management.
version: 0.2.0
---

# /clawhub -- ClawHub Skill Manager

You are the ClawHub skill manager for this RightClaw agent.

## When to Activate

Activate this skill when the user:
- Wants to install a skill (e.g., "install a skill", "add skill", "get me a skill for...")
- Wants to find or search for skills (e.g., "find skills", "search ClawHub", "what skills are available")
- Mentions ClawHub, skill packages, or skill management
- Wants to remove or uninstall a skill (e.g., "remove skill", "uninstall skill")
- Wants to list installed skills (e.g., "list installed skills", "what skills do I have")
- Wants to update skills (e.g., "update skills", "upgrade skill")

## Configuration

**Base URL:** `https://clawhub.ai` (override with `CLAWHUB_REGISTRY` env var if set)

**Skill install path:** `skills/<name>/` (per-agent isolation -- each agent has its own skills directory)

**Registry file:** `skills/installed.json` (tracks all installed skills for this agent)

## Commands

### search \<query\>

Search the ClawHub registry for skills matching a query.

1. Construct the search URL:
   ```
   curl -sS "https://clawhub.ai/api/v1/skills?q=<query>"
   ```
   If `CLAWHUB_REGISTRY` is set, use that as the base URL instead.

2. Parse the JSON response. Expected format:
   ```json
   {
     "success": true,
     "data": [
       {
         "slug": "TheSethRose/agent-browser",
         "name": "Agent Browser",
         "description": "Browse the web autonomously",
         "version": "1.2.0",
         "author": "TheSethRose",
         "tags": ["web", "browser", "automation"]
       }
     ]
   }
   ```

3. Present results as a formatted table:

   | Name | Description | Author | Version |
   |------|-------------|--------|---------|
   | Agent Browser | Browse the web autonomously | TheSethRose | 1.2.0 |

4. Ask the user if they want to install any of the results.

**Error handling:**
- If the API returns a non-200 status or is unreachable: inform the user that ClawHub is unavailable. Suggest manual git clone as a fallback: `git clone <repo-url> skills/<name>/`
- If 429 (rate limited): suggest authenticating with a ClawHub token.
- If 401 (unauthorized): suggest setting a `CLAWHUB_TOKEN` env var or running `clawhub login`.
- If response JSON is malformed: report the error and suggest trying again.

### install \<slug\>

Install a skill by its ClawHub slug (e.g., `TheSethRose/agent-browser`).

**Step 1: Fetch metadata**

```
curl -sS "https://clawhub.ai/api/v1/skills/<slug>"
```

Parse the response to get `version`, `description`, and `name`. If the API is unreachable, suggest manual git clone as fallback.

**Step 2: Download ZIP**

```
curl -sSL -o /tmp/clawhub-skill.zip "https://clawhub.ai/api/v1/download/<slug>/<version>"
```

**Step 3: Extract to skill directory**

```bash
mkdir -p /tmp/clawhub-extract
unzip -o /tmp/clawhub-skill.zip -d /tmp/clawhub-extract
# Move contents to skills/<name>/
mkdir -p skills/<name>
# Handle both cases: root directory present or files at root level
# If a single directory exists at root, move its contents; otherwise move all files
cp -r /tmp/clawhub-extract/*/. skills/<name>/ 2>/dev/null || cp -r /tmp/clawhub-extract/. skills/<name>/
rm -rf /tmp/clawhub-extract /tmp/clawhub-skill.zip
```

**Step 4: Policy gate audit**

Before activating the skill, audit its permissions. Read the downloaded `skills/<name>/SKILL.md` frontmatter and check for `metadata.openclaw` and `metadata.openshell` sections.

Check each permission category:

| Category | Frontmatter field | Verification |
|----------|-------------------|--------------|
| Required binaries | `metadata.openclaw.requires.bins` | Run `which <bin>` for each -- is it in PATH? |
| Required env vars | `metadata.openclaw.requires.env` | Run `echo $VAR` for each -- is it set? |
| Network access | `metadata.openshell.network` | Check agent's `policy.yaml` -- are these domains allowed? |
| Filesystem access | `metadata.openshell.filesystem` | Check agent's `policy.yaml` -- is this access level allowed? |

**If ANY check fails: BLOCK the installation.**

Display a permissions audit table:

| Permission | Required | Status |
|------------|----------|--------|
| Binary: python3 | Yes | MISSING -- not in PATH |
| Env: OPENAI_API_KEY | Yes | MISSING -- not set |
| Network: api.openai.com | Yes | BLOCKED -- not in policy.yaml |
| Filesystem: read-write | Yes | OK -- allowed by policy |

Tell the user:
> Installation blocked. The skill requires permissions that your agent does not have. Update your agent's `policy.yaml` to allow the missing permissions, then retry the installation.

**If all checks pass** (or the skill has no special requirements): proceed to Step 5.

**Step 5: Register in installed.json**

Read `skills/installed.json`. If it does not exist, create it with `{}`.

Add an entry for the new skill:

```json
{
  "<name>": {
    "version": "1.2.0",
    "slug": "TheSethRose/agent-browser",
    "installed_at": "2026-03-22T10:00:00Z",
    "path": "skills/<name>"
  }
}
```

All timestamps MUST use UTC ISO 8601 format with the `Z` suffix. Generate the timestamp with:
```bash
date -u +"%Y-%m-%dT%H:%M:%SZ"
```

Write the updated JSON back to `skills/installed.json`.

**Step 6: Confirm installation**

Report to the user:
> Installed **<name>** v<version> from ClawHub. The skill is now available at `skills/<name>/`.

### remove \<name\>

Remove an installed skill by name.

1. Read `skills/installed.json`. If it does not exist, inform the user that no skills are tracked.

2. Check if `<name>` exists in the registry.
   - If not found: inform the user that the skill is not installed (or not tracked). Check if `skills/<name>/` exists on disk -- if so, suggest it may have been manually installed.

3. If found:
   - Delete the `skills/<name>/` directory: `rm -rf skills/<name>/`
   - Remove the entry from `skills/installed.json`
   - Write the updated JSON back to `skills/installed.json`

4. Confirm removal:
   > Removed **<name>** and unregistered it from installed.json.

### list

List all installed skills for this agent.

1. Read `skills/installed.json`. If it does not exist, start with an empty registry.

2. Scan the `skills/` directory for subdirectories containing a `SKILL.md` file. Include any skills found on disk but not in `installed.json` (these are manually installed).

3. Present a table:

   | Name | Version | Source | Installed |
   |------|---------|--------|-----------|
   | agent-browser | 1.2.0 | clawhub | 2026-03-22T10:00:00Z |
   | my-custom-skill | - | manual | - |

   - Source is `clawhub` if tracked in `installed.json`, `manual` if found on disk only.
   - Version and installed date come from `installed.json`; show `-` for manually installed skills.

## Error Handling

- **Network errors:** If `curl` fails or the API is unreachable, inform the user and suggest manual git clone: `git clone <repo-url> skills/<name>/`
- **401 Unauthorized:** Suggest authenticating with a ClawHub token via `CLAWHUB_TOKEN` env var.
- **429 Rate Limited:** Suggest waiting or authenticating to increase rate limits.
- **500 Server Error:** Report the error and suggest retrying later.
- **Malformed JSON:** Report the parsing error with the raw response snippet.
- **Missing installed.json:** Create an empty `{}` file automatically -- this is normal for a fresh agent.
- **ZIP extraction failures:** Report the error, clean up temp files, suggest manual download.

## Important Rules

1. All timestamps MUST use UTC ISO 8601 format with the `Z` suffix (e.g., `2026-03-22T10:00:00Z`).
2. Never install a skill without running the policy gate audit first. No exceptions.
3. Never auto-expand the agent's policy.yaml to accommodate a skill's requirements. The user must explicitly update policy.yaml.
4. Each agent has its own skills directory and installed.json -- no shared or global skill location.
5. If `CLAWHUB_REGISTRY` is set, use it as the base URL for all API calls instead of `https://clawhub.ai`.
6. Skills installed manually (via git clone) appear in `list` output but are not managed for updates.
