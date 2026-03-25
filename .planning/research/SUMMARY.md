# Project Research Summary

**Project:** RightClaw v2.2 Skills Registry
**Domain:** Multi-agent runtime CLI — skills registry integration, env var injection
**Researched:** 2026-03-25
**Confidence:** HIGH

## Executive Summary

RightClaw v2.2 is a tightly-scoped delta milestone on top of a fully-shipped v2.1. The core work is three bounded changes: rename the built-in skill from `/clawhub` to `/skills` to match the primary registry (skills.sh), rework the policy gate to check CC-native sandbox state instead of dead OpenShell/policy.yaml references, and add an `env:` section to `agent.yaml` for per-agent environment variable injection. No new Rust crates are needed — every capability maps to existing workspace dependencies (serde, serde-saphyr, minijinja, stdlib). The recommended execution order is env var injection first (clean vertical slice through types → codegen → template), rename second (pure mechanical refactor), then SKILL.md policy gate update last (content-only, blocked only on rename completing).

The primary technical risks concentrate in the env var injection feature. Two security issues require attention from day one: shell quoting of user-supplied values (unquoted injection creates command execution in the wrapper before the CC sandbox takes effect) and the ordering of env var export relative to the HOME override in the wrapper script. A third issue is a pre-existing data-loss bug in `install_builtin_skills()` — it unconditionally resets `installed.json` to `{}` on every `rightclaw up`, wiping the user's skill registry. This must be fixed before v2.2 ships any user-facing `/skills install` capability or users will lose their installed skills on every restart.

The skills.sh registry (Vercel, launched Jan 2026, 90k+ skills) uses the agentskills.io standard format, which does NOT include `metadata.openclaw` fields. The current policy gate silently passes all skills.sh skills because it only checks `metadata.openclaw.requires.*`. The v2.2 policy gate rework must handle both formats: `compatibility` field for agentskills.io skills and `metadata.openclaw.requires.*` for ClawHub skills. ClawHub should be treated as a secondary fallback requiring explicit opt-in, not automatic fallback, given the ClawHavoc supply chain incident (341 confirmed malicious skills, Feb 2026).

---

## Key Findings

### Recommended Stack

No new dependencies are required for v2.2. The existing workspace fully covers all capability needs. `serde` + `serde-saphyr` handle the new `env: HashMap<String, String>` field deserialization; `minijinja` extends the `agent-wrapper.sh.j2` template with a for-loop export block; `std::env::var()` handles `${VAR}` passthrough expansion at launch time; `reqwest` + `serde_json` are already in-workspace if Rust-side skills.sh search is added later.

**Core technologies (unchanged from v2.1, relevant to v2.2 changes):**
- `serde` + `serde-saphyr`: Deserialize new `env: HashMap<String, String>` field — both already in workspace
- `minijinja`: Extend wrapper template with per-agent env var export block — already in workspace
- `stdlib std::env::var`: Expand `${VAR}` references at `rightclaw up` time — no crate needed
- `reqwest` + `serde_json`: Available for future Rust-side skills.sh `GET /api/search` if needed

**What NOT to add:** `envy` (maps host env into Rust structs — wrong direction), `shellexpand` (overkill for a 5-line pattern match), any new HTTP client, any Rust-side git clone for skill install (agent handles this via `npx skills`).

### Expected Features

v2.2 is defined by three P1 features. All are complete-or-nearly-complete in design; implementation is straightforward.

**Must have (table stakes for v2.2 launch):**
- Rename `/clawhub` → `/skills` — naming must match primary registry; `/clawhub` confuses users who know skills.sh
- Policy gate rework (CC-native sandbox) — current gate checks dead OpenShell fields; silently passes all skills.sh skills
- `env:` in `agent.yaml` + shell wrapper export — unblocks entire class of skills requiring API keys (browser-use, etc.)

**Should have (v2.2.x after validation):**
- ClawHub explicit secondary fallback — backward compat with OpenClaw ecosystem; requires `--registry clawhub` opt-in, not auto-fallback
- `compatibility` field display in policy audit — show freeform field from agentskills.io format during install

**Defer to v2.3+:**
- Secretspec / `.secrets.yaml` — per SEED-006, requires design work; `env: VAR: "${VAR}"` passthrough covers immediate need
- Auto-update cron for skills via `/rightcron` — requires careful built-in skill overwrite logic
- Tech Leads Club as third verified registry source
- `allowed-tools` frontmatter enforcement — requires CC upstream support for the experimental field

**Hard anti-features (do not build):**
- Auto-expand sandbox for skill requirements — violates security model
- `--force` to skip policy gate — defeats the product's value proposition
- Global skill registry shared across agents — breaks per-agent isolation
- Symlinked skills from host `.claude/skills/` — concurrent agent race conditions

### Architecture Approach

All v2.2 changes are additive modifications to existing components. No new modules, no new crates, no new templates are required. The implementation footprint is six files modified and one directory renamed.

**Major components and their v2.2 changes:**
1. `agent/types.rs` / `AgentConfig` — Add `env: HashMap<String, String>` with `#[serde(default)]`; backward-compatible with existing agent.yaml files
2. `codegen/shell_wrapper.rs` / `generate_wrapper()` — Extract `env_vars: Vec<(k,v)>` from config, add to minijinja context; no signature change
3. `templates/agent-wrapper.sh.j2` — Insert `{% for key, value in env_vars %} export {{key}}="{{value}}" {% endfor %}` AFTER identity vars, BEFORE `export HOME=` (ordering is load-bearing)
4. `codegen/skills.rs` — Rename `SKILL_CLAWHUB` const to `SKILL_SKILLS`, update `include_str!` path, update install tuple from `clawhub/SKILL.md` to `skills/SKILL.md`
5. `init.rs` — Update println path string from `clawhub/SKILL.md` to `skills/SKILL.md`
6. `skills/clawhub/` → `skills/skills/` (git mv) — SKILL.md content already targets skills.sh; add `metadata.requires.*` alias and ClawHub fallback section after rename

**Key architectural pattern — env vars are late-bound.** The wrapper emits `export VAR="${VALUE}"` verbatim. Bash `${}` expansion happens when the wrapper executes, not at codegen time. Resolving at codegen time would write secret values into the generated wrapper file on disk — a security violation. This pattern must be preserved.

### Critical Pitfalls

1. **Shell injection via unquoted env var values** — Values with spaces, quotes, or `$` break the bash wrapper or enable command injection before CC sandbox takes effect. Prevention: implement `shell_quote(s: &str) -> String` helper (`'...'` quoting with `'\''` for embedded single quotes) in `generate_wrapper()`. Must ship with the feature, not as a follow-up. Same fix needed for the existing `startup_prompt` variable.

2. **`installed.json` reset on every `rightclaw up`** — `install_builtin_skills()` unconditionally writes `{}` to `installed.json`, wiping all user-installed skill registry entries on every restart. Fix: check if file exists before writing. This is a confirmed data-loss bug requiring a regression test before the fix.

3. **Policy gate is a no-op for skills.sh (primary registry)** — Gate checks `metadata.openclaw.requires.*` only; skills.sh uses agentskills.io format without that namespace. All skills.sh skills silently pass the gate regardless of actual requirements. Fix: gate must also read `compatibility` field and check CC-native sandbox settings for any skill, regardless of format.

4. **Env var ordering relative to HOME override** — Injecting `env:` vars after `export HOME=` causes values referencing `$HOME` to resolve to the agent dir, not the host home. Fix: inject env vars AFTER identity vars but BEFORE `export HOME=`. This ordering is load-bearing and must be enforced.

5. **Rename completeness — four locations must update atomically** — The `clawhub` path string appears in the filesystem (skills/ dir), Rust const, install tuple, and test assertions. Missing any one location results in silent path mismatch. Fix: `rg -l "clawhub"` before commit, `cargo test` catches path errors. Also: `rightclaw up` must remove legacy `.claude/skills/clawhub/` directories from prior agent dirs.

---

## Implications for Roadmap

Based on combined research, a three-phase execution order is strongly recommended.

### Phase 1: Env Var Injection + Data-Loss Bug Fix

**Rationale:** Env var injection is the highest-value feature and the pre-existing `installed.json` data-loss bug must be fixed in the same phase — before any user installs skills that would be wiped on `rightclaw up`. This phase is a clean vertical slice through data model → codegen → template with no dependency on rename work. Security requirements (shell quoting) are part of this phase, not an afterthought. TDD approach: write failing tests for `AgentConfig` with `env:` field, wrapper export output, and `installed.json` preservation before writing any implementation code.

**Delivers:** `env:` section in `agent.yaml`, per-agent env vars exported in shell wrapper before `exec claude`, `installed.json` preserved across `rightclaw up` restarts, shell-quoting protection for user-supplied values.

**Addresses:** `env:` injection (P1), `installed.json` preservation (prerequisite for all skill management).

**Avoids:** Pitfall #1 (shell injection — `shell_quote()` helper first), Pitfall #2 (installed.json reset — regression test + fix), Pitfall #4 (env var ordering — AFTER identity vars, BEFORE HOME override), Pitfall #6 (secret leakage — document `${VAR}` forwarding pattern, not literal embedding).

### Phase 2: Skill Rename (clawhub → skills)

**Rationale:** Pure mechanical refactor with no logic. Can technically run in parallel with Phase 1 since files are disjoint, but sequenced here because Phase 3 is blocked on rename completing. The rename is more dangerous than it looks — four locations must update atomically and old agent dirs need cleanup.

**Delivers:** `/skills` slash command (was `/clawhub`), correct naming alignment with skills.sh, migration cleanup of old `.claude/skills/clawhub/` directories in agent dirs on `rightclaw up`.

**Addresses:** Rename P1 requirement.

**Avoids:** Pitfall #8 (rename completeness — use `rg -l "clawhub"` to find all four locations, update atomically, `cargo test` verifies path correctness), UX regression for existing users with `/clawhub` in agent memory.

### Phase 3: Policy Gate Rework + SKILL.md Update

**Rationale:** Content-only change to `skills/skills/SKILL.md`. Blocked on Phase 2 (file must be at new path before editing). The gate rework is the most nuanced part — must handle two SKILL.md formats (agentskills.io vs. metadata.openclaw) and check CC-native sandbox state. No Rust compilation required.

**Delivers:** Policy gate that checks `sandbox.enabled`, `sandbox.network.allowedDomains`, `sandbox.filesystem.allowWrite` from the agent's `.claude/settings.json`; handles skills.sh skills (agentskills.io format) and ClawHub skills (metadata.openclaw format); shows which registry served a skill; requires explicit opt-in for ClawHub fallback.

**Addresses:** Policy gate rework (P1), ClawHub explicit secondary fallback (P2 — can be scoped here or deferred).

**Avoids:** Pitfall #5 (gate must not silently pass skills.sh skills), Pitfall #9 (ClawHub fallback must show source, require opt-in), Pitfall #7 (document that `registry.npmjs.org` and `github.com` must be in allowed_domains for skill install).

### Phase Ordering Rationale

- Phase 1 first: data-loss bug is a correctness prerequisite; env var injection has no dependencies on rename; security requirements must be solved before the feature ships.
- Phase 2 second: pure refactor, prerequisite for Phase 3.
- Phase 3 last: SKILL.md content-only; safe to iterate on separately; no Rust compilation.
- Phases 1 and 2 can run in parallel on separate branches (disjoint files), but sequential execution reduces integration risk.

### Research Flags

Phases with standard patterns (skip research-phase):
- **Phase 2 (Rename):** Pure refactor — `rg` + path update + `cargo test`. No research needed.

Phases needing focused attention during plan creation:
- **Phase 1 (Env var injection):** Shell quoting edge cases deserve explicit test case design upfront. The `startup_prompt` quoting bug should be fixed in the same pass — check current test coverage before planning scope.
- **Phase 3 (Policy gate):** The gate logic in SKILL.md is instruction text executed by a Claude Code agent — JSON paths in `.claude/settings.json` should be re-verified against the actual schema before writing. Pitfall #7 (npx blocked by sandbox) needs a concrete resolution: add `registry.npmjs.org`/`github.com` to default `allowed_domains` in `generate_settings()`, or document as required manual override with actionable error message.

---

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Verified from codebase inspection — all v2.2 capabilities covered by existing deps; no new crates needed |
| Features | HIGH | skills.sh CLI verified from GitHub source; agentskills.io spec fetched directly; CC sandbox fields from docs; existing codebase audited |
| Architecture | HIGH | All integration points derived from direct codebase inspection (types.rs, shell_wrapper.rs, agent-wrapper.sh.j2, skills.rs, init.rs) |
| Pitfalls | HIGH | Most pitfalls confirmed via direct code audit (installed.json bug verified in source; shell quoting gap verified in template; four-location rename risk confirmed by rg) |

**Overall confidence:** HIGH

### Gaps to Address

- **npx sandbox access:** Concrete decision needed — add `registry.npmjs.org` and `github.com` to default `allowed_domains` in `generate_settings()`, or require explicit agent.yaml override with actionable error when absent. Research leaves this open; must be resolved in Phase 1 or Phase 3 planning.
- **skills.sh `compatibility` field:** The field is freeform prose (not machine-parseable per agentskills.io spec). Decision needed: display it during install as informational, or skip for v2.2.
- **`startup_prompt` shell quoting:** Pitfall #1 notes the existing `startup_prompt` variable has the same quoting bug. Phase 1 scope should explicitly include or exclude it — do not leave as known-but-deferred.
- **ClawHub post-incident API state:** Whether ClawHub added auth/rate limiting post-ClawHavoc is unverified. If implementing explicit fallback in Phase 3, verify the API still accepts unauthenticated requests and handle 429/403 explicitly.

---

## Sources

### Primary (HIGH confidence)
- `crates/rightclaw/src/agent/types.rs` — AgentConfig struct, deny_unknown_fields, confirmed no `env:` field yet
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — generate_wrapper(), template context pattern
- `crates/rightclaw/src/codegen/skills.rs` — SKILL_CLAWHUB const, install_builtin_skills(), confirmed unconditional `{}` write to installed.json
- `crates/rightclaw/src/init.rs` — clawhub path references at lines 173, 250
- `templates/agent-wrapper.sh.j2` — wrapper ordering, identity vars, HOME override position
- `skills/clawhub/SKILL.md` — current policy gate implementation
- `https://raw.githubusercontent.com/vercel-labs/skills/main/src/find.ts` — verified search endpoint `GET /api/search`, no auth required
- `https://agentskills.io/specification` — official SKILL.md format spec, `compatibility` field is freeform prose
- `https://code.claude.com/docs/en/sandboxing` — CC sandbox settings.json field schema

### Secondary (MEDIUM confidence)
- `https://skills.sh/docs/cli` — CLI command list (partial content returned by WebFetch)
- `https://github.com/openclaw/clawhub/issues/522` — ClawHub requires.env not extracted into registry metadata bug
- `metadata.openclaw` format from openclaw/clawhub docs/skill-format.md
- ClawHavoc attack details (341 malicious skills, Feb 2026) — from WebSearch

### Tertiary (requires validation during execution)
- ClawHub API state post-ClawHavoc (auth/rate limiting) — unverified
- `npx skills` behavior inside CC bubblewrap sandbox (network access) — inferred from CC sandbox docs, not directly tested

---
*Research completed: 2026-03-25*
*Ready for roadmap: yes*
