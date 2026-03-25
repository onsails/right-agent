# Feature Research: Skills Registry (v2.2)

**Domain:** Agent skill manager — multi-registry skill lifecycle management for Claude Code agents
**Researched:** 2026-03-25
**Confidence:** HIGH (skills.sh CLI verified via GitHub/docs; agentskills.io spec fetched directly; ClawHub metadata.openclaw from openclaw/clawhub source; CC sandbox fields from code.claude.com docs)

---

## Context: What Already Exists

The existing `/clawhub` skill (in `skills/clawhub/SKILL.md`) already implements the core skill lifecycle using skills.sh as primary registry. It supports search, install, remove, list, and update via `npx skills`. The policy gate audits `metadata.openclaw` frontmatter against the agent's `.claude/settings.json`. This is v2.2's starting point, not a blank slate.

**v2.2 scope is three bounded changes:**
1. Rename `/clawhub` → `/skills` (skill file + Rust constants + install paths)
2. Rework policy gate (drop OpenShell/policy.yaml refs, check CC-native sandbox fields)
3. Add `env:` section to `agent.yaml` + shell wrapper export

---

## Registry Landscape

### skills.sh (Primary)

**What it is:** Vercel's open agent skills ecosystem directory. 90,000+ skills indexed, telemetry-based leaderboard. Launched January 20, 2026.

**CLI:** `npx skills` — no install required, runs via npx. Commands:

| Command | Flags | Notes |
|---------|-------|-------|
| `add <owner/repo>` | `--list`, `--skill <name>`, `-a <agent>`, `--copy`, `-y`, `--all`, `-g` | Primary install command |
| `find [query]` | — | Interactive fzf-style search OR keyword |
| `list` / `ls` | `-g` | Lists installed skills |
| `remove [name]` | `-a <agent>`, `--all`, `-g` | Uninstall |
| `check` | — | Detect available updates |
| `update` | — | Upgrade all installed |
| `init` | — | Create new SKILL.md template |

**Agent targets:** `claude-code`, `cursor`, `opencode`, and 40+ others. `-a claude-code` installs to `.claude/skills/`. `-a claude` is an accepted alias (per current SKILL.md).

**Discovery fallback:** If `npx skills find` fails, GitHub topic search `--topic=agent-skill` is the documented fallback.

**Public HTTP API:** No documented public REST API. The leaderboard data (skills.sh/leaderboard) is web-only. Programmatic search = `npx skills find` or GitHub API.

**Security:** No built-in verification (unlike ClawHub's VirusTotal gate). Telemetry collects skill name + timestamp by default; `DISABLE_TELEMETRY=1` disables.

### ClawHub (Secondary / Fallback)

**What it is:** OpenClaw's registry. 13,729+ community skills as of Feb 2026. Vector (semantic) search powered by OpenAI embeddings. Had ClawHavoc supply chain attack (Feb 2026) — 341 confirmed malicious skills, now VirusTotal-gated.

**API:** HTTP REST at clawhub.ai. Semantic search endpoint. Skills have SHA-256 hashes in frontmatter for verification. VirusTotal Code Insight verdict per skill.

**`metadata.openclaw` format (ClawHub extension, not in agentskills.io standard):**
```yaml
metadata:
  openclaw:
    requires:
      env:
        - TODOIST_API_KEY
      bins:
        - curl
      bins_any:
        - chrome
        - chromium
    primaryEnv: TODOIST_API_KEY
    install:
      - kind: brew
        formula: jq
        bins: [jq]
      - kind: node
        package: typescript
        bins: [tsc]
```
Fields: `requires.env` (must-have env vars), `requires.bins` (all required), `requires.bins_any` (at least one), `requires.config` (config file paths), `install` (auto-install specs with kinds: `brew`, `node`, `go`, `uv`), `primaryEnv` (key env var).

**Known bug (Issue #522, Feb 2026):** `requires.env` not extracted into registry metadata — scanner always shows "Required env vars: none." The frontmatter is the authoritative source; do not rely on registry metadata for policy checks. Read SKILL.md directly after download.

### agentskills.io Standard (SKILL.md Format)

Anthropic-published open standard (December 2025). Supported by 16+ agents: Claude Code, Cursor, Gemini CLI, OpenAI Codex, GitHub Copilot, VS Code, OpenHands, Goose, etc.

**Frontmatter fields (authoritative from spec, fetched directly):**

| Field | Required | Constraints | Notes |
|-------|----------|-------------|-------|
| `name` | Yes | 1-64 chars, lowercase+hyphens, no consecutive `--`, matches dir name | Primary key |
| `description` | Yes | 1-1024 chars, what+when | Loaded at startup for all skills (~100 tokens) |
| `license` | No | Free text | — |
| `compatibility` | No | 1-500 chars, freeform | Environment requirements (binaries, network, etc.) — NOT machine-parseable |
| `metadata` | No | Arbitrary key-value map | Where `metadata.openclaw` and other extensions live |
| `allowed-tools` | No | Space-delimited tool list | Experimental: `Bash(git:*) Read` |

**`metadata` is a free-form catch-all.** `metadata.openclaw` is a ClawHub extension, not part of the agentskills.io standard. The `compatibility` field is the standard's way to declare environment requirements — it is freeform prose, not machine-parseable.

**Progressive disclosure:** `name` + `description` loaded at startup; full SKILL.md body loaded on activation. Keep under 500 lines; reference external files for detail.

---

## Feature Landscape

### Table Stakes (Users Expect These)

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| Search skills.sh registry | Any skill manager has search | LOW | `npx skills find <query>` already in SKILL.md; GitHub topic fallback documented |
| Install by owner/repo slug | Standard package manager pattern | LOW | `npx skills add <slug> -a claude --copy -y` already implemented |
| Remove installed skill | Symmetry with install | LOW | `npx skills remove <name> -a claude -y` already implemented |
| List installed skills | Users need inventory visibility | LOW | `npx skills list -a claude` + scan `.claude/skills/` already implemented |
| `installed.json` tracking per agent | Know what's installed, track source | LOW | Already implemented; source field (`skills.sh` vs `manual`) already present |
| Skill renamed `/clawhub` → `/skills` | Name must match primary registry (skills.sh, not ClawHub) | LOW | Rename: `skills/clawhub/` dir → `skills/skills/`, `SKILL_CLAWHUB` const in `codegen/skills.rs`, `init.rs` print statement, tests |
| Manual install fallback | npx not always available | LOW | `git clone` fallback already in SKILL.md |
| Policy gate before any skill activation | Security hygiene — block skills missing deps before they run | MEDIUM | Gate exists; needs rework to check CC-native sandbox instead of policy.yaml |

### Differentiators (Competitive Advantage)

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| Policy gate checks CC-native sandbox | Show actual sandbox state (`settings.json` `allowedDomains`, `allowWrite`) not dead OpenShell refs | MEDIUM | Read agent's `.claude/settings.json` → check `sandbox.network.allowedDomains` and `sandbox.filesystem.allowWrite`; drop all `policy.yaml` path mentions |
| `env:` section in `agent.yaml` | Per-agent env var injection — skills that need API keys work without host env exposure | LOW-MEDIUM | Add `env: Option<IndexMap<String, String>>` to `AgentConfig`; export in shell wrapper before `exec claude`; `${VAR}` host env passthrough syntax |
| Shell wrapper env export block | Env vars available to all bash commands run by skills (inside sandbox) | LOW | Add export loop in `agent-wrapper.sh.j2` after identity vars, before HOME override |
| ClawHub as secondary/fallback | Backward compat with OpenClaw ecosystem (13,729+ skills, semantic search) | LOW | `gh search` fallback partially done; explicit ClawHub HTTP fallback in install/search flow |
| Skills update command | Drift management for installed skills | LOW | `npx skills update` already in SKILL.md |
| `compatibility` field display in audit | Show freeform compatibility string during install — user sees what skill expects | LOW | Parse `compatibility` from frontmatter; display in policy gate audit table (freeform only, no enforcement) |

### Anti-Features (Commonly Requested, Often Problematic)

| Feature | Why Requested | Why Problematic | Alternative |
|---------|---------------|-----------------|-------------|
| Secrets management / vault integration | API keys in agent.yaml are plaintext | secretspec/vault adds external dependency, complex UX, requires design — scope creep for v2.2 | Use `env: VAR: "${VAR}"` passthrough — value comes from host env, never stored in agent.yaml. SEED-006 explicitly flags secretspec as future design work; defer to v2.3+ |
| Auto-expand sandbox for skill requirements | Convenience — install skill and it just works | Violates RightClaw's security model: users must explicitly consent to capability expansion. Expanding sandbox silently defeats the entire point of the policy gate | Block install, show audit table, tell user exactly what to add to `agent.yaml sandbox:` section |
| Global skill registry (shared across agents) | Save disk space, single update | Breaks agent isolation — HOME override means each agent has own `.claude/`; shared skills create cross-agent state leakage | Per-agent `.claude/skills/` is the correct model. SKILL.md files are small; per-agent storage is acceptable |
| Auto-install binary dependencies from `metadata.openclaw.install` | `install` field has brew/node/go/uv spec | Executing arbitrary install steps is privilege escalation — changes host system state outside sandbox; requires trust in skill author | Block install, show what binary is missing, show the `install` spec for user to run manually |
| Semantic search via HTTP API | Better discovery than keyword search | skills.sh has no public REST API; ClawHub has one but was supply-chain compromised; building an API client for a compromised registry is risky | `npx skills find` (interactive fzf) + GitHub topic search covers search adequately |
| `--force` flag to skip policy gate | Power users want to bypass | Defeats the purpose; RightClaw's value is enforced hygiene; one `--force` install creating a credential-stealing skill destroys user trust | No force flag. If a check is a false positive, fix the check. If user wants unsafe install, they can copy manually |
| Symlinking skills from host `.claude/skills/` into agent | "Share my personal skills across agents" | Symlink races with concurrent agents; changes in one agent affect others; defeats per-agent isolation | Copy at install time via `--copy` flag (already enforced). Document why symlinking is not supported |

---

## Feature Dependencies

```
[Rename /clawhub → /skills (Rust)]
    └──required before──> [All downstream tests pass]
    └──touches──> [skills/clawhub/ dir rename]
    └──touches──> [SKILL_CLAWHUB const in codegen/skills.rs]
    └──touches──> [init.rs print statement]
    └──touches──> [codegen/skills.rs tests asserting "clawhub/SKILL.md"]

[env: section in AgentConfig (Rust types)]
    └──required before──> [Shell wrapper env export]
    └──required before──> [env vars available to skill commands at runtime]
    └──touches──> [agent/types.rs AgentConfig struct]
    └──touches──> [deny_unknown_fields — must add field or it rejects env:]
    └──touches──> [agent-wrapper.sh.j2 template]
    └──touches──> [codegen/shell_wrapper.rs context builder]

[Policy gate rework (SKILL.md instruction update)]
    └──depends on──> [Read .claude/settings.json at skill install time]
    └──independent of──> [Rename]
    └──independent of──> [env: injection]
    └──touches──> [skills/clawhub/SKILL.md policy gate section]

[ClawHub secondary fallback]
    └──enhances──> [Search command]
    └──independent of──> [Policy gate rework]
    └──independent of──> [Rename]
```

### Dependency Notes

- **Rename is the foundation:** `SKILL_CLAWHUB` const rename and directory rename (`skills/clawhub/` → `skills/skills/`) must happen atomically. `init.rs` has a print statement referencing `agents/right/.claude/skills/clawhub/SKILL.md` — must update. Tests in `codegen/skills.rs` assert `clawhub/SKILL.md` exists — all must update together.
- **env: injection is two parts:** (1) `AgentConfig` gets `env` field — `deny_unknown_fields` will REJECT agent.yaml files with `env:` until the field is added to the struct. (2) wrapper template iterates the map. Both must ship together.
- **Shell wrapper env export order matters:** Env vars must be exported AFTER the identity vars (GIT_CONFIG_GLOBAL, SSH_AUTH_SOCK, etc.) but BEFORE the `export HOME=` line. Reason: values using `${VAR}` syntax resolve from the parent process env (process-compose), which is correct. After HOME override, `~` in a value would resolve against agent dir — acceptable if that's what the user specifies.
- **Policy gate rework is SKILL.md only:** The gate logic lives in the SKILL.md instruction file (executed by Claude). Rework = update the markdown instructions. No Rust changes required.
- **`deny_unknown_fields` on AgentConfig is the primary risk:** Any agent.yaml using `env:` before the Rust struct is updated will fail validation with "unknown field". Must update struct + tests atomically.

---

## MVP Definition

### Launch With (v2.2)

These are the three scoped features from the milestone. All are bounded; none require new infrastructure.

- [ ] **Rename `/clawhub` → `/skills`** — Name must match primary registry. Low complexity. Rename: directory, Rust const, init.rs print statement, tests. Update `install_builtin_skills()` to install `skills/SKILL.md` not `clawhub/SKILL.md`.
- [ ] **Policy gate rework** — Drop dead `policy.yaml` references. Read `sandbox.network.allowedDomains` and `sandbox.filesystem.allowWrite` from agent's `.claude/settings.json`. Without this the gate reports against a model that no longer exists (OpenShell).
- [ ] **`env:` in `agent.yaml` + shell wrapper export** — Add `env` field to `AgentConfig`, iterate in wrapper template. Unblocks skills requiring API keys (browser-use, etc.). Without this, agents cannot use skills needing env vars not in process-compose's inherited environment.

### Add After Validation (v2.2.x)

- [ ] **ClawHub explicit secondary fallback** — Already partially implemented via `gh search` fallback. Add explicit ClawHub HTTP API call in install/search flow when skills.sh fails. Gate behind a `--source clawhub` opt-in flag to be explicit about using the compromised registry.
- [ ] **`compatibility` field display in policy audit** — Show freeform `compatibility` string from SKILL.md during install. Low effort, adds transparency for skills using the standard field instead of `metadata.openclaw`.

### Future Consideration (v2.3+)

- [ ] **Secretspec / `.secrets.yaml`** — Per SEED-006, design needed. Vault integration, `source: env|file|vault` abstraction. Blocked on design, not implementation.
- [ ] **Auto-update cron for skills** — `npx skills update` on a schedule via `/rightcron`. Requires careful handling of built-in skill overwrite logic.
- [ ] **Tech Leads Club as verified source** — Snyk-scanned, content-hashed skills. Add as third registry option behind `--source tls` flag.
- [ ] **`allowed-tools` frontmatter enforcement** — When a SKILL.md declares `allowed-tools: Bash(git:*) Read`, enforce that the skill only uses declared tools. Requires CC support for this experimental field.

---

## Feature Prioritization Matrix

| Feature | User Value | Implementation Cost | Priority |
|---------|------------|---------------------|----------|
| Rename `/clawhub` → `/skills` | HIGH (naming accuracy, removes confusion) | LOW (rename const, dir, tests) | P1 |
| Policy gate rework (CC-native sandbox) | HIGH (correctness — old gate checks dead fields) | MEDIUM (update SKILL.md instructions, read settings.json) | P1 |
| `env:` in `agent.yaml` + wrapper export | HIGH (unblocks class of skills needing API keys) | LOW-MEDIUM (add Rust field + template loop) | P1 |
| ClawHub explicit secondary fallback | MEDIUM (backward compat with OpenClaw ecosystem) | LOW (partially implemented) | P2 |
| `compatibility` field display | LOW (informational only) | LOW (parse + display) | P2 |
| Secretspec / vault | MEDIUM (security hygiene) | HIGH (design + implementation) | P3 |

---

## Policy Gate: Precise Change Specification

The policy gate lives in the SKILL.md instructions (Claude executes them). Current v2.1 state vs required v2.2 state:

| Check | Current (broken) | Reworked (correct) |
|-------|-----------------|-------------------|
| Required binaries | `which <bin>` | Keep as-is (still valid) |
| Required env vars | `echo $VAR` | Keep as-is (still valid) |
| Network access | Check `policy.yaml` allowedDomains | Check agent's `.claude/settings.json` → `sandbox.network.allowedDomains` |
| Filesystem access | Check `policy.yaml` allowWrite | Check agent's `.claude/settings.json` → `sandbox.filesystem.allowWrite` |
| Policy file reference | "Check agent's `policy.yaml`" in SKILL.md text | Remove all policy.yaml mentions; replace with settings.json path |
| Sandbox enabled check | None | Add: check `sandbox.enabled` is `true`; warn if sandbox is off |

**What the reworked gate reads from `.claude/settings.json` (relative to agent cwd):**
- `sandbox.enabled` — is sandbox active at all?
- `sandbox.network.allowedDomains` — array of allowed domains
- `sandbox.filesystem.allowWrite` — array of writable paths
- `sandbox.autoAllowBashIfSandboxed` — auto-run mode (informational for user)

**What `metadata.openclaw.requires` supplies (from downloaded SKILL.md):**
- `requires.env` — env var names → check via `echo $VAR` (unchanged)
- `requires.bins` — binary names → check via `which` (unchanged)
- `requires.bins_any` — at-least-one binary check (unchanged)
- `requires.network` — domain list → check against `sandbox.network.allowedDomains` (NEW source)
- `requires.filesystem` — path list → check against `sandbox.filesystem.allowWrite` (NEW source)

Gate logic is unchanged (block on any miss, display audit table). Only the source for network/filesystem data changes.

---

## env: Section Design

**`agent.yaml` shape:**
```yaml
env:
  BROWSER_USE_API_KEY: "${BROWSER_USE_API_KEY}"   # passthrough from host env (resolved by shell)
  CUSTOM_VAR: "literal-value"                      # literal string
  HOME_RELATIVE: "~/some/path"                     # resolved against host HOME before wrapper override
```

**Rust type addition to `AgentConfig` (agent/types.rs):**
Use `IndexMap<String, String>` (ordered, reproducible) not `HashMap` (nondeterministic). Field is optional with empty default to avoid breaking existing agent.yaml files without `env:`.

```rust
/// Per-agent environment variables injected into the shell wrapper.
/// Values support `${VAR}` host-env passthrough (resolved by bash at runtime).
#[serde(default)]
pub env: IndexMap<String, String>,
```

Note: `deny_unknown_fields` on `AgentConfig` will REJECT any agent.yaml with `env:` until this field is added. This is the breaking change risk — must add field before any agent.yaml adoption.

**Shell wrapper template block position (agent-wrapper.sh.j2):**
Insert after identity env var block, before `export HOME=`:

```bash
# Per-agent env vars from agent.yaml env: section
{% for key, value in env %}
export {{ key }}="{{ value }}"
{% endfor %}
```

Position rationale: (1) After identity vars so they're already set if needed. (2) Before `HOME=` override so `${VAR}` references resolve from process-compose's inherited environment (the host env), not the agent dir context. This is the intended behavior for passthrough.

---

## Competitor Feature Analysis

| Feature | npx skills (skills.sh) | clawhub CLI | RightClaw /skills |
|---------|----------------------|-------------|------------------|
| Search | `npx skills find <q>` | `clawhub search <q>` (semantic) | `npx skills find` + GitHub fallback |
| Install | `npx skills add <owner/repo>` | `clawhub install <slug>` | `npx skills add` + git clone fallback |
| Remove | `npx skills remove` | `clawhub uninstall` | `npx skills remove` + manual fallback |
| Update | `npx skills update` | `clawhub update --all` | `npx skills update` |
| Policy gate | None | VirusTotal + SHA256 hash | CC-native sandbox check (custom) |
| Multi-registry | No (skills.sh only) | No (ClawHub only) | Yes (skills.sh primary, ClawHub fallback) |
| Install tracking | Lock file (tree SHA) | Registry metadata | `installed.json` per agent |
| Env var injection | Out of scope | Out of scope | `env:` in agent.yaml |
| Security post-ClawHavoc | No verification | VirusTotal-gated | Policy gate checks CC sandbox state |

---

## Sources

- [agentskills.io/specification](https://agentskills.io/specification) — official spec, fetched directly (HIGH confidence)
- [github.com/vercel-labs/skills](https://github.com/vercel-labs/skills) — skills CLI source, fetched directly (HIGH confidence)
- [skills.sh/docs/cli](https://skills.sh/docs/cli) — CLI docs (MEDIUM confidence — partial content returned by WebFetch)
- [code.claude.com/docs/en/sandboxing](https://code.claude.com/docs/en/sandboxing) — CC sandbox fields schema, fetched directly (HIGH confidence)
- [openclaw/clawhub#522](https://github.com/openclaw/clawhub/issues/522) — ClawHub `requires.env` not extracted into registry bug (MEDIUM confidence, from WebSearch)
- `metadata.openclaw` format from openclaw/clawhub docs/skill-format.md (MEDIUM confidence, from WebSearch summary)
- SEED-005, SEED-006 — project seeds (HIGH confidence, read directly)
- `skills/clawhub/SKILL.md` — current implementation (HIGH confidence, read directly)
- `crates/rightclaw/src/agent/types.rs` — current `AgentConfig` struct with `deny_unknown_fields` (HIGH confidence, read directly)
- `templates/agent-wrapper.sh.j2` — current wrapper template with identity var order (HIGH confidence, read directly)
- `crates/rightclaw/src/codegen/skills.rs` — current `install_builtin_skills()` with clawhub path (HIGH confidence, read directly)
- [vercel-labs/skills GitHub README](https://github.com/vercel-labs/skills) — CLI command list (HIGH confidence, fetched directly)
- [ClawHavoc attack details](https://github.com/openclaw/clawhub) — supply chain attack context (MEDIUM confidence, WebSearch)

---
*Feature research for: RightClaw v2.2 Skills Registry milestone*
*Researched: 2026-03-25*
