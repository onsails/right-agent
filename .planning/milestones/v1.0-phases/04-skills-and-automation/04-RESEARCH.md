# Phase 4: Skills and Automation - Research

**Researched:** 2026-03-22
**Domain:** Claude Code skills (SKILL.md), ClawHub HTTP API, CronSync reconciliation, shell wrapper codegen
**Confidence:** MEDIUM

## Summary

This phase delivers two Claude Code skills (`/clawhub` and `/cronsync`) plus a minor Rust codegen update for the system prompt file. The skills are pure markdown (SKILL.md format) that instruct Claude Code how to interact with the ClawHub registry API and how to reconcile cron job specs. The Rust changes are limited to: (1) generating a system prompt markdown file at `run/<agent>-system.md`, and (2) updating the shell wrapper template to pass it via `--append-system-prompt-file`.

Research verified ClawHub's REST API endpoints, Claude Code's cron tool capabilities and constraints, SKILL.md authoring best practices, and lock file concurrency patterns. One open question remains: whether `--append-system-prompt-file` can be specified multiple times on the same command line. The safe fallback is to concatenate system prompt content into a single file.

**Primary recommendation:** Write both skills as concise SKILL.md files (under 500 lines each) following Anthropic's progressive disclosure pattern. Use the verified ClawHub API v1 endpoints. For CronSync, implement the reconciliation logic as inline markdown instructions since Claude Code will execute the file operations and cron tool calls natively.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Research actual ClawHub API endpoints during plan-phase research (not known yet)
- **D-02:** Existing `skills/clawhub/SKILL.md` is a good starting point -- refine with real API details
- **D-03:** If ClawHub API is unreachable, skill suggests manual git clone as fallback
- **D-04:** Skills install to `$RIGHTCLAW_HOME/agents/<agent>/skills/<name>/` -- per-agent isolation
- **D-05:** No shared/global skill location in v1
- **D-06:** `skills/installed.json` tracks installed skills per agent
- **D-07:** When a skill requests permissions the agent's policy doesn't grant -> BLOCK installation
- **D-08:** Show exactly what permissions are needed vs what's allowed -- user must manually update policy.yaml first
- **D-09:** No auto-expansion of policy -- security-first means user explicitly decides
- **D-10:** Gate reads `metadata.openclaw.requires` from SKILL.md frontmatter (bins, env vars, network)
- **D-11:** CronSync is a Claude Code skill (`/cronsync`) that uses CC's native CronCreate/CronList/CronDelete tools
- **D-12:** Reconciliation: reads `crons/*.yaml` as desired state, CronList as actual state, state.json as mapping
- **D-13:** Creates missing jobs, deletes orphaned jobs, recreates changed jobs (hash-based change detection)
- **D-14:** Lock-file concurrency: heartbeat-based with configurable TTL, UTC ISO 8601 timestamps
- **D-15:** Lock files at `crons/.locks/<name>.json`, state at `crons/state.json` -- both gitignored
- **D-16:** RightClaw generates `$RIGHTCLAW_HOME/run/<agent>-system.md` with CronSync bootstrap instructions
- **D-17:** Shell wrapper passes this as additional `--append-system-prompt-file` -- user can't modify (regenerated on each `up`)
- **D-18:** System prompt instructs agent to run `/cronsync` on startup if `crons/` directory exists with YAML specs
- **D-19:** The system prompt file is read OUTSIDE the sandbox by the wrapper script -- no policy change needed
- **D-20:** Shell wrapper template needs updating to support second `--append-system-prompt-file` for system prompt
- **D-21:** Codegen (Rust) needs to generate the system prompt file at `run/<agent>-system.md`

### Claude's Discretion
- ClawHub API error handling and retry logic
- CronSync skill conversation style (verbose vs quiet)
- Lock file stale detection edge cases
- System prompt wording for CronSync bootstrap

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SKLM-01 | `/clawhub` skill can search ClawHub registry by name or description via HTTP API | ClawHub REST API `GET /api/v1/skills?q=<query>` verified via DeepWiki + official docs |
| SKLM-02 | `/clawhub` skill can install a skill by slug -- downloads to agent's `skills/` directory | `GET /api/v1/skills/<slug>` for metadata, `GET /api/v1/download/<slug>/<version>` for ZIP |
| SKLM-03 | `/clawhub` skill can uninstall a skill by name -- removes from `skills/` directory | Pure filesystem operation + installed.json update -- no API research needed |
| SKLM-04 | `/clawhub` skill can list installed skills for the current agent | Read installed.json + scan skills/ directory -- no API research needed |
| SKLM-05 | Policy gate audits skill permissions from SKILL.md frontmatter before activation | `metadata.openclaw.requires` schema verified: `bins`, `env`, `network` fields |
| SKLM-06 | Skills use standard ClawHub SKILL.md format with YAML frontmatter -- drop-in compatible | SKILL.md format verified: `name`, `description` required; `version`, `metadata` optional |
| CRON-01 | CronSync reads `crons/*.yaml` specs as desired state | YAML spec format from seed.md: `schedule`, `lock_ttl`, `max_turns`, `prompt` |
| CRON-02 | CronSync reconciles desired state against live cron jobs via state.json mapping | CronList tool returns job IDs, schedules, prompts -- sufficient for reconciliation |
| CRON-03 | CronSync creates/deletes/recreates jobs | CronCreate (5-field cron + prompt + recurrence flag), CronDelete (by 8-char ID) verified |
| CRON-04 | Lock-file concurrency with heartbeat-based TTL | Heartbeat pattern researched; TOCTOU risk documented as pitfall |
| CRON-05 | All timestamps in lock files use UTC ISO 8601 format (suffix `Z`) | Claude Code uses local timezone for cron; lock files MUST use UTC explicitly |
| CRON-06 | Cron YAML specs support `schedule`, `lock_ttl`, `max_turns`, and `prompt` fields | All fields documented in seed.md; `max_turns` maps to `--max-turns` CLI flag |
</phase_requirements>

## Standard Stack

This phase is primarily markdown skill authoring (SKILL.md files) with minor Rust code changes for codegen.

### Core (Skill Files -- no dependencies)
| Artifact | Format | Purpose | Why |
|----------|--------|---------|-----|
| `skills/clawhub/SKILL.md` | Claude Code SKILL.md | ClawHub skill manager | Instructions for Claude to interact with ClawHub HTTP API |
| `skills/cronsync/SKILL.md` | Claude Code SKILL.md | CronSync reconciliation skill | Instructions for Claude to reconcile cron specs with live jobs |

### Rust Changes (existing dependencies only)
| Module | File | Purpose | Why |
|--------|------|---------|-----|
| codegen::shell_wrapper | `shell_wrapper.rs` | Add system_prompt_path to template context | Pass second `--append-system-prompt-file` |
| codegen (new fn) | `system_prompt.rs` or inline | Generate system prompt markdown | CronSync bootstrap instructions |
| agent-wrapper.sh.j2 | Template | Add conditional second prompt file | Wire system prompt into wrapper |

No new crate dependencies required for this phase.

### ClawHub API (External)
| Endpoint | Method | Purpose | Auth |
|----------|--------|---------|------|
| `/api/v1/skills?q=<query>` | GET | Search skills by name/description | Optional Bearer token |
| `/api/v1/skills/<slug>` | GET | Get skill metadata, resolve latest version | Optional |
| `/api/v1/download/<slug>/<version>` | GET | Download skill ZIP | Optional |
| `/api/v1/skills/bulk` | POST | Batch fingerprint check for sync | Bearer token |
| `/.well-known/clawhub.json` | GET | Registry discovery | None |

**Base URL:** `https://clawhub.ai` (overridable via `CLAWHUB_REGISTRY` env var)

### Claude Code Cron Tools (Built-in)
| Tool | Purpose | Parameters |
|------|---------|------------|
| CronCreate | Schedule a new task | 5-field cron expression, prompt text, recurrence flag |
| CronList | List all scheduled tasks | None -- returns all tasks with IDs, schedules, prompts |
| CronDelete | Cancel a task | 8-character task ID |

## Architecture Patterns

### Skill Directory Structure
```
skills/
├── clawhub/
│   └── SKILL.md          # /clawhub skill (search, install, remove, list)
└── cronsync/
    └── SKILL.md          # /cronsync skill (reconciliation)
```

### Per-Agent Installed Skills
```
$RIGHTCLAW_HOME/agents/<agent>/
├── skills/
│   ├── installed.json     # Registry of installed skills
│   └── <skill-name>/
│       └── SKILL.md       # Downloaded skill
├── crons/
│   ├── *.yaml             # Cron job specs (desired state)
│   ├── state.json         # Job ID mapping (gitignored)
│   └── .locks/            # Lock files (gitignored)
│       └── <name>.json
└── ...
```

### Pattern 1: SKILL.md Progressive Disclosure
**What:** Keep SKILL.md under 500 lines. Only `name` and `description` are loaded at startup; full content loaded on invocation.
**When to use:** Always -- this is how Claude Code skills work.
**Key rules:**
- Description must be specific enough for Claude to know when to trigger
- Write in third person: "Manages ClawHub skills..." not "I manage..."
- Name: lowercase, hyphens only, max 64 chars
- No reserved words: "anthropic", "claude"

### Pattern 2: CronSync Reconciliation Loop
**What:** Declarative state reconciliation: desired (YAML files) vs actual (CronList output) vs tracked (state.json).
**When to use:** Every CronSync invocation.
**Algorithm:**
1. Read all `crons/*.yaml` files -> desired state map
2. Call CronList -> actual state (live jobs)
3. Read `crons/state.json` -> tracked state (name -> job_id + schedule + prompt_hash)
4. For each desired entry:
   - Not in tracked -> CronCreate, add to state.json
   - In tracked but schedule/prompt changed (hash mismatch) -> CronDelete old + CronCreate new, update state.json
   - In tracked and unchanged -> skip
5. For each tracked entry not in desired -> CronDelete, remove from state.json
6. Write updated state.json

### Pattern 3: Lock File with Heartbeat
**What:** File-based concurrency control using heartbeat timestamps.
**When to use:** When CronSync wraps cron job prompts with guard logic.
**Algorithm:**
1. Check `crons/.locks/<name>.json`
2. If exists and `heartbeat` is within `lock_ttl` -> skip (still running)
3. If exists and `heartbeat` exceeds `lock_ttl` -> stale, delete lock
4. If not exists -> create lock with current UTC timestamp
5. Execute prompt
6. Periodically update heartbeat during execution
7. Delete lock on completion

**Lock file format:**
```json
{"heartbeat": "2026-03-22T10:05:00Z"}
```

### Pattern 4: System Prompt Generation (Rust)
**What:** Generate a markdown file at `run/<agent>-system.md` during `rightclaw up`.
**When to use:** For every agent, regenerated on each `up` invocation.
**Integration:**
- Generate content with CronSync bootstrap instructions
- Write to `run/<agent>-system.md`
- Pass path to shell wrapper template as `system_prompt_path`
- Template conditionally adds `--append-system-prompt-file` if path exists

### Anti-Patterns to Avoid
- **Over-engineering SKILL.md:** Skills are instructions, not code. Don't try to make SKILL.md do validation or complex logic -- Claude handles that natively.
- **Storing state in SKILL.md:** State belongs in `state.json` and `installed.json`, never in the skill file itself.
- **Assuming CronList returns consistent ordering:** Job IDs are the stable identifier; match by ID from state.json, not by position.
- **Hardcoding ClawHub URL:** Use `https://clawhub.ai` as default but document `CLAWHUB_REGISTRY` override.
- **Auto-expanding policy on install:** Decision D-09 explicitly forbids this. BLOCK and explain what's needed.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Cron scheduling | Custom scheduler | Claude Code's CronCreate/CronList/CronDelete | Built-in tools, session-scoped, handles timing/jitter |
| ZIP extraction | Custom unzip code | Claude's native Bash tool (`unzip` command) | Skill instructs Claude to run `unzip`; no Rust code needed |
| HTTP requests to ClawHub | Rust HTTP client in this phase | Claude's native WebFetch or Bash `curl` | Skills instruct Claude to make HTTP calls; not Rust code |
| YAML parsing of cron specs | Custom parser | Claude reads YAML natively | Skills are instructions; Claude interprets YAML files |
| Hash computation for change detection | Rust hash function | Claude's Bash tool (`sha256sum` or similar) | Hashing happens in the skill instructions, not Rust |

**Key insight:** This phase's skills are SKILL.md files -- instructions that Claude follows. The "implementation" is writing clear instructions, not writing code that runs independently. The only actual code is the Rust codegen changes for system prompt generation and shell wrapper update.

## Common Pitfalls

### Pitfall 1: Cron Task 3-Day Expiry
**What goes wrong:** All Claude Code cron jobs auto-expire after 3 days. CronSync creates jobs that silently disappear.
**Why it happens:** Session-scoped cron has a built-in 3-day TTL. No notification when tasks expire.
**How to avoid:** CronSync must be designed to handle this gracefully. On each reconciliation run, it recreates any expired jobs from the YAML specs. The system prompt should trigger CronSync on every agent startup (D-18).
**Warning signs:** State.json has entries for jobs that CronList doesn't return.

### Pitfall 2: Timezone Mismatch Between Cron and Locks
**What goes wrong:** Claude Code interprets cron expressions in local timezone, but lock files use UTC (D-05/CRON-05).
**Why it happens:** Two different time systems coexist.
**How to avoid:** CronSync instructions must explicitly tell Claude to use UTC for lock file timestamps (`date -u +%Y-%m-%dT%H:%M:%SZ`) while understanding that cron schedules fire in local time. The lock_ttl comparison must use UTC consistently.
**Warning signs:** Locks appear stale when they shouldn't be (timezone offset).

### Pitfall 3: TOCTOU Race in Lock File Check
**What goes wrong:** Two cron firings check the lock file simultaneously, both see "no lock", both proceed.
**Why it happens:** File check and file creation are not atomic.
**How to avoid:** This is an inherent limitation of file-based locking in a single-process Claude Code session. In practice, Claude Code is single-threaded -- scheduled prompts fire between turns, never concurrently. CronSync's lock files protect against overlapping runs of the SAME cron job (slow previous run + new trigger), not true concurrent access. The design is sound for this use case.
**Warning signs:** None expected -- Claude Code's scheduling model prevents true concurrent execution.

### Pitfall 4: --append-system-prompt-file May Not Support Multiple Invocations
**What goes wrong:** Shell wrapper tries two `--append-system-prompt-file` flags; only one takes effect.
**Why it happens:** CLI flag parsing may not support repeated flags. Official docs don't confirm multi-use.
**How to avoid:** Safe approach: concatenate IDENTITY.md content and system prompt content into a single file at `run/<agent>-system.md`, then use ONE `--append-system-prompt-file` pointing to it. OR: use `--append-system-prompt-file` for IDENTITY.md and `--append-system-prompt "$(cat run/<agent>-system.md)"` for the system prompt. Testing required during implementation.
**Warning signs:** Agent starts but doesn't run `/cronsync` on startup.

### Pitfall 5: ClawHub API Authentication
**What goes wrong:** Skill assumes unauthenticated access works for all endpoints.
**Why it happens:** Some endpoints require Bearer token auth. Search and download may work without auth but could be rate-limited.
**How to avoid:** Skill should try without auth first, then suggest `clawhub login` or manual token if rate-limited or 401. Document `CLAWHUB_REGISTRY` and token storage.
**Warning signs:** 401 or 429 responses from ClawHub API.

### Pitfall 6: Skill Description Under-triggering
**What goes wrong:** Claude doesn't invoke `/clawhub` or `/cronsync` when the user expects it.
**Why it happens:** Description field is too vague or doesn't cover enough trigger scenarios.
**How to avoid:** Write "pushy" descriptions that cover multiple phrasings. Anthropic's guidance: "Claude has a tendency to under-trigger skills." Include explicit trigger conditions: "Use when the user mentions installing skills, finding skills, ClawHub, skill management, removing skills, or listing installed skills."
**Warning signs:** User says "install a skill" and Claude doesn't activate `/clawhub`.

### Pitfall 7: Max 50 Cron Tasks Per Session
**What goes wrong:** CronSync tries to create more than 50 jobs and fails silently.
**Why it happens:** Claude Code has a hard limit of 50 scheduled tasks per session.
**How to avoid:** CronSync should count existing tasks before creating new ones. If approaching limit, warn the user.
**Warning signs:** CronCreate calls fail or are silently dropped.

## Code Examples

### ClawHub SKILL.md Frontmatter
```yaml
---
name: clawhub
description: >-
  Manages ClawHub skills for this RightClaw agent. Searches the ClawHub registry,
  installs skills by slug, removes installed skills, and lists all installed skills.
  Use when the user wants to find, install, remove, update, or list Claude Code skills,
  or mentions ClawHub, skill packages, or skill management.
version: 0.2.0
---
```

### CronSync SKILL.md Frontmatter
```yaml
---
name: cronsync
description: >-
  Reconciles scheduled cron jobs with YAML spec files in the crons/ directory.
  Creates missing jobs, deletes orphaned jobs, and recreates changed jobs using
  CronCreate/CronList/CronDelete tools. Use when the user mentions cron jobs,
  scheduled tasks, CronSync, or when starting up with a crons/ directory present.
version: 0.1.0
---
```

### ClawHub API Search Response (expected format)
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

### CronSync state.json Format
```json
{
  "deploy-check": {
    "job_id": "4e9fed67",
    "schedule": "*/5 * * * *",
    "prompt_hash": "a1b2c3d4e5f6g7h8"
  }
}
```

### Lock File Format
```json
{"heartbeat": "2026-03-22T10:05:00Z"}
```

### Policy Gate Check (metadata.openclaw.requires)
```yaml
# ClawHub skill frontmatter with openclaw metadata
---
name: some-skill
description: Does something
version: 1.0.0
metadata:
  openclaw:
    requires:
      bins: [python3, node]
      env: [OPENAI_API_KEY, GITHUB_TOKEN]
    primaryEnv: OPENAI_API_KEY
  openshell:
    network: [api.github.com, api.openai.com]
    filesystem: read-only
---
```

The policy gate should check:
1. `bins` -> are these binaries available in the sandbox PATH?
2. `env` -> are these environment variables set?
3. `network` (from openshell) -> does agent's policy.yaml allow these domains?
4. `filesystem` (from openshell) -> does agent's policy.yaml allow this access level?

If any check fails -> BLOCK installation, show what's missing vs what's allowed.

### Shell Wrapper Template Update (agent-wrapper.sh.j2)
```bash
# Current: single --append-system-prompt-file for IDENTITY.md
# Updated: add conditional second --append-system-prompt-file for system prompt

{% if not no_sandbox %}
exec openshell sandbox create \
  --policy "{{ policy_path }}" \
  --name "rightclaw-{{ agent_name }}" \
  -- claude \
    --append-system-prompt-file "{{ identity_path }}" \
    {% if system_prompt_path %}--append-system-prompt-file "{{ system_prompt_path }}" \
    {% endif %}--dangerously-skip-permissions \
    {% if channels %}--channels {{ channels }} \
    {% endif %}--prompt "{{ start_prompt }}"
{% else %}
# WARNING: Running without sandbox (--no-sandbox mode)
exec claude \
  --append-system-prompt-file "{{ identity_path }}" \
  {% if system_prompt_path %}--append-system-prompt-file "{{ system_prompt_path }}" \
  {% endif %}--dangerously-skip-permissions \
  {% if channels %}--channels {{ channels }} \
  {% endif %}--prompt "{{ start_prompt }}"
{% endif %}
```

**IMPORTANT:** If `--append-system-prompt-file` does not support repeated use, the fallback is to generate a combined file that includes both IDENTITY.md content and system prompt content, then use a single `--append-system-prompt-file` pointing to the combined file. This must be tested during implementation.

### System Prompt Content (for CronSync bootstrap)
```markdown
## RightClaw System Instructions

On startup, check if the `crons/` directory exists in your agent directory.
If it contains `.yaml` files, run `/cronsync` to reconcile scheduled tasks.

This ensures all declared cron jobs are active after agent restart or session expiry.
```

### Rust: System Prompt Generation Function
```rust
// Source: new function in codegen module
pub fn generate_system_prompt(agent: &AgentDef) -> String {
    let crons_dir = agent.path.join("crons");
    let has_crons = crons_dir.is_dir();

    let mut prompt = String::from("## RightClaw System Instructions\n\n");
    if has_crons {
        prompt.push_str(
            "On startup, check if the `crons/` directory exists in your agent directory.\n\
             If it contains `.yaml` files, run `/cronsync` to reconcile scheduled tasks.\n\n\
             This ensures all declared cron jobs are active after agent restart or session expiry.\n"
        );
    }
    prompt
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `serde_yaml` for YAML | `serde-saphyr` | March 2024 (archived) | Already using serde-saphyr in project |
| `/loop` for all scheduling | CronCreate/CronList/CronDelete tools | Claude Code v2.1.72 (early 2026) | CronSync uses native tools, not `/loop` |
| `--append-system-prompt` (inline) | `--append-system-prompt-file` (file) | Claude Code ~2025 | Cleaner for multi-line prompts |
| ClawHub was "clawhub.com" | Now "clawhub.ai" | ~2026 | Use clawhub.ai as base URL |

**Deprecated/outdated:**
- `clawhub.com` domain: the registry moved to `clawhub.ai`
- `metadata.clawdbot` and `metadata.clawdis`: legacy aliases for `metadata.openclaw` -- still supported by ClawHub but use `metadata.openclaw` for new skills

## Open Questions

1. **Can `--append-system-prompt-file` be specified multiple times?**
   - What we know: Official docs show it used once. The `--plugin-dir` flag explicitly says "Repeat the flag for multiple directories." `--append-system-prompt-file` docs do NOT say this.
   - What's unclear: Whether repeating it causes the second to overwrite the first or both to append.
   - Recommendation: Test during implementation. If it doesn't work, use the concatenated file approach (generate a single combined file with IDENTITY.md + system prompt content).

2. **ClawHub API rate limits for unauthenticated requests**
   - What we know: ClawHub has per-IP rate limiting. Bearer token auth exists.
   - What's unclear: Exact rate limit thresholds. Whether search/download work without auth.
   - Recommendation: Implement without auth first; add error handling for 429/401 with suggestion to authenticate.

3. **ClawHub API response format for search endpoint**
   - What we know: JSON response with `success` and `data` fields. Skills have slug, name, description, version.
   - What's unclear: Exact pagination parameters, total count field, result ordering.
   - Recommendation: Implement with basic `?q=` parameter; handle pagination if results are truncated.

4. **ZIP extraction path for downloaded skills**
   - What we know: Skills are downloaded as ZIP files containing SKILL.md + supporting files.
   - What's unclear: Whether the ZIP has a root directory or files are at the root level.
   - Recommendation: Extract to a temp directory first, then move contents to the target skill directory. Handle both cases (root dir present / files at root).

## Sources

### Primary (HIGH confidence)
- [Claude Code CLI Reference](https://code.claude.com/docs/en/cli-reference) - `--append-system-prompt-file` flag, `--max-turns`, all CLI flags
- [Claude Code Scheduled Tasks](https://code.claude.com/docs/en/scheduled-tasks) - CronCreate/CronList/CronDelete tools, 50 task limit, 3-day expiry, jitter, timezone behavior
- [Anthropic Skill Authoring Best Practices](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices) - SKILL.md structure, naming, descriptions, progressive disclosure, anti-patterns
- [Claude Code Skills Docs](https://code.claude.com/docs/en/skills) - Skill format, frontmatter fields, deployment locations

### Secondary (MEDIUM confidence)
- [ClawHub DeepWiki](https://deepwiki.com/openclaw/clawhub) - API endpoints, metadata schema, download workflow (verified against GitHub repo structure)
- [ClawHub GitHub](https://github.com/openclaw/clawhub) - Source code confirming API routes in convex/httpApiV1.ts

### Tertiary (LOW confidence)
- ClawHub API response format details - inferred from DeepWiki analysis, not directly tested against live API
- Rate limiting thresholds - mentioned in source but specific limits not documented publicly
- ZIP archive structure - inferred from publish flow, not verified by downloading a real package

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - no new dependencies; skills are pure SKILL.md; Rust changes use existing minijinja + template pattern
- Architecture: HIGH - CronSync design is fully specified in seed.md; skill format well-documented by Anthropic
- ClawHub API: MEDIUM - endpoints verified via DeepWiki/GitHub but not tested against live service
- Lock file concurrency: MEDIUM - pattern is well-understood but TOCTOU mitigation relies on Claude Code's single-threaded scheduling model
- Pitfalls: HIGH - cron expiry, timezone mismatch, and under-triggering are well-documented gotchas

**Research date:** 2026-03-22
**Valid until:** 2026-04-05 (ClawHub API may change; cron tools stable since v2.1.72)
