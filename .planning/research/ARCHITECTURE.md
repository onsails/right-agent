# Architecture Research

**Domain:** v2.2 Skills Registry — env var injection + /skills rename integration
**Researched:** 2026-03-25
**Confidence:** HIGH (derived from direct codebase inspection)

## Context

This is a subsequent-milestone research file. The existing architecture is fully built and shipped (v2.1). This document maps the integration points for the two v2.2 concerns:

1. **Env var injection** — `env:` section in `agent.yaml`, exported in the shell wrapper before `exec claude`
2. **`/skills` rename** — `skills/clawhub/` directory + `SKILL_CLAWHUB` const renamed; SKILL.md already rewritten for skills.sh

The SKILL.md at `skills/clawhub/SKILL.md` is already rewritten to target skills.sh (name: `skills`, all commands reference `npx skills`). The rename is purely mechanical.

## System Overview

```
agent.yaml                      skills/clawhub/SKILL.md
  env:                  RENAME  skills/skills/SKILL.md
    KEY: value            |
       |                  v
       |          codegen/skills.rs
       v          install_builtin_skills()
agent/types.rs     SKILL_SKILLS const
  AgentConfig        install path: skills/SKILL.md
  .env field
       |
       v
codegen/shell_wrapper.rs
  generate_wrapper()
  env_vars: Vec<(k,v)>
       |
       v
templates/agent-wrapper.sh.j2
  for loop: export K="V"
       |
       v
~/.rightclaw/agents/<name>/agent-wrapper.sh
  [env vars visible to exec claude]
```

## Current Architecture (relevant paths only)

```
rightclaw/
├── crates/rightclaw/src/
│   ├── agent/
│   │   └── types.rs              AgentConfig struct — ADD env field
│   ├── codegen/
│   │   ├── mod.rs                re-exports (no change)
│   │   ├── shell_wrapper.rs      generate_wrapper() — ADD env_vars to context
│   │   └── skills.rs             install_builtin_skills() — RENAME const + path
│   └── init.rs                   RENAME println! path string
├── skills/
│   └── clawhub/SKILL.md          RENAME to skills/skills/SKILL.md
└── templates/
    └── agent-wrapper.sh.j2       ADD for-loop export block
```

## Integration Points: Env Var Injection

### 1. `agent/types.rs` — Add `env` field to `AgentConfig`

Add after `telegram_user_id`:

```rust
/// Per-agent environment variables exported by the shell wrapper before exec claude.
/// Values support bash variable syntax: "${HOST_VAR}" expands at wrapper execution time.
#[serde(default)]
pub env: std::collections::HashMap<String, String>,
```

`HashMap<String, String>` is the right type. No wrapper struct needed — the section is flat key-value pairs. `#[serde(deny_unknown_fields)]` already exists on `AgentConfig`; adding a named field with `#[serde(default)]` is backward-compatible (existing `agent.yaml` files without `env:` continue to deserialize to an empty map).

### 2. `codegen/shell_wrapper.rs` — Pass env vars to template context

`generate_wrapper()` currently collects `agent.config` fields and passes them to minijinja. Add:

```rust
let env_vars: Vec<(String, String)> = agent
    .config
    .as_ref()
    .map(|c| c.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    .unwrap_or_default();
```

Then include in the `context!` macro:

```rust
tmpl.render(context! {
    // ... existing fields ...
    env_vars => env_vars,
})
```

No signature change to `generate_wrapper()` — `env_vars` derives from `agent: &AgentDef` which is already a parameter.

### 3. `templates/agent-wrapper.sh.j2` — Export block before HOME override

Insert after the existing identity env var block (lines 7-12) but before `export HOME=` (line 15):

```jinja2
{% for key, value in env_vars %}
export {{ key }}="{{ value }}"
{% endfor %}
```

**Ordering is load-bearing.** The export block must go before `export HOME=` because:
- Values containing `${SOME_VAR}` expand at bash runtime in the agent process, not at codegen time — ordering does not matter for that
- Values that reference the real HOME (e.g., path strings) would need to be captured before HOME is overridden — this is a defensive ordering
- The identity vars block (GIT_*, SSH_AUTH_SOCK, ANTHROPIC_API_KEY) must remain first since those explicitly capture from the pre-override environment

Full ordering in the resulting wrapper:
```
1. Identity env vars (GIT_*, SSH_AUTH_SOCK, ANTHROPIC_API_KEY)   [existing]
2. agent.yaml env: vars                                            [NEW]
3. export HOME="{{ working_dir }}"                                 [existing]
4. claude binary resolution                                        [existing]
5. exec "$CLAUDE_BIN" ...                                          [existing]
```

Quoting: `{{ value }}` is double-quoted in the template. Values containing `${VAR}` references (e.g., `"${BROWSER_USE_API_KEY}"`) will be bash-expanded at wrapper execution time. `set -euo pipefail` in the wrapper means an unset referenced variable causes immediate exit — correct fail-fast behavior.

## Integration Points: `/skills` Rename

### 1. `skills/clawhub/` → `skills/skills/`

Git rename: `git mv skills/clawhub skills/skills`. The SKILL.md content already targets skills.sh — no content changes needed in this step.

### 2. `crates/rightclaw/src/codegen/skills.rs`

Two line changes:

```rust
// Before:
const SKILL_CLAWHUB: &str = include_str!("../../../../skills/clawhub/SKILL.md");
// After:
const SKILL_SKILLS: &str = include_str!("../../../../skills/skills/SKILL.md");

// Before:
("clawhub/SKILL.md", SKILL_CLAWHUB),
// After:
("skills/SKILL.md", SKILL_SKILLS),
```

The install path changes from `.claude/skills/clawhub/SKILL.md` to `.claude/skills/skills/SKILL.md`. Claude Code discovers skills by scanning `.claude/skills/*/SKILL.md` — the directory name becomes the slash command name, so `skills/` becomes `/skills`.

### 3. `crates/rightclaw/src/init.rs`

One cosmetic println! on line 173:
```rust
// Before:
println!("  agents/right/.claude/skills/clawhub/SKILL.md  (skills.sh manager)");
// After:
println!("  agents/right/.claude/skills/skills/SKILL.md  (/skills manager)");
```

### 4. Test updates in `codegen/skills.rs`

Three existing tests assert on `clawhub/SKILL.md` path strings — change to `skills/SKILL.md`. No logic changes.

Same for `init.rs` test `init_creates_default_agent_files` which asserts:
```rust
assert!(agents_dir.join(".claude/skills/clawhub/SKILL.md").exists(), ...)
// change to:
assert!(agents_dir.join(".claude/skills/skills/SKILL.md").exists(), ...)
```

## Policy Gate in SKILL.md

The current SKILL.md policy gate (Step 3 of install) already checks `settings.json` rather than `policy.yaml`. No OpenShell references remain. Two minor content updates needed:

1. Add `metadata.requires.*` as an alias for `metadata.openclaw.requires.*` — agentskills.io format skills don't use the `openclaw` namespace
2. Add ClawHub as secondary fallback in the search section (already noted in SEED-005 as a requirement)

This is a SKILL.md content edit only — no Rust changes.

## Component Boundaries

| Component | What Changes | Change Type |
|-----------|-------------|-------------|
| `agent/types.rs` | Add `env: HashMap<String, String>` to `AgentConfig` | Additive, backward-compat |
| `codegen/shell_wrapper.rs` | Extract env_vars from config, add to context | Logic addition |
| `templates/agent-wrapper.sh.j2` | Add for-loop export block before HOME override | Template addition |
| `codegen/skills.rs` | Rename const + include_str! path + install tuple | Mechanical rename |
| `skills/clawhub/` (directory) | Rename to `skills/skills/` | Git mv |
| `init.rs` | Update println! path string | Cosmetic |
| `skills/skills/SKILL.md` | Add metadata.requires.* alias + ClawHub fallback | Content edit |

## Files: New vs Modified

**Modified (no new files needed):**
- `crates/rightclaw/src/agent/types.rs`
- `crates/rightclaw/src/codegen/shell_wrapper.rs`
- `crates/rightclaw/src/codegen/skills.rs`
- `crates/rightclaw/src/init.rs`
- `templates/agent-wrapper.sh.j2`

**Renamed:**
- `skills/clawhub/` → `skills/skills/` (one git mv)

**Content-edited (not renamed):**
- `skills/skills/SKILL.md` — policy gate metadata alias + ClawHub fallback

**No new Rust modules, no new crates, no new templates.**

## Data Flow: rightclaw up with env vars

```
agent.yaml
  env:
    BROWSER_USE_API_KEY: "${BROWSER_USE_API_KEY}"
    CUSTOM_VAR: "literal"
         |
         v
AgentConfig.env = HashMap {
  "BROWSER_USE_API_KEY" => "${BROWSER_USE_API_KEY}",
  "CUSTOM_VAR" => "literal",
}     |
      v
generate_wrapper(agent, ...)
  env_vars: Vec<(k,v)>
      |
      v
agent-wrapper.sh.j2 renders:
  export BROWSER_USE_API_KEY="${BROWSER_USE_API_KEY}"
  export CUSTOM_VAR="literal"
      |
      v
~/.rightclaw/agents/<name>/agent-wrapper.sh (written to disk)
      |
      v
process-compose executes wrapper in host environment
  bash expands ${BROWSER_USE_API_KEY} from host env at runtime
      |
      v
exec claude  [sees env vars in its process environment]
```

## Architectural Patterns

### Pattern 1: Env vars are late-bound (expanded at exec, not at codegen)

**What:** `generate_wrapper()` emits `export KEY="${VALUE}"` verbatim. Bash `${}` expansion happens when the wrapper executes, not when rightclaw generates the script.

**When to use:** Always — this is the only correct approach for env vars that may reference secrets.

**Why:** The wrapper script is generated at `rightclaw up` time in the Rust process environment. It executes later in the process-compose subprocess environment. These may differ. More critically, resolving `${API_KEY}` at codegen time would write the secret value into the generated wrapper file on disk — a security violation.

### Pattern 2: Install path = slash command name

**What:** The directory name under `.claude/skills/<name>/` becomes the Claude Code slash command `/name`. The const in `skills.rs` includes the SKILL.md from the matching source directory.

**When to use:** Any built-in skill addition follows this: add `skills/<name>/SKILL.md`, add `const SKILL_<NAME>` pointing to it, add tuple to `built_in_skills` array.

**Trade-off:** The outer `skills/` directory (the Rust workspace folder) and the inner `skills/` install target (the skill's own name) share the same name — `skills/skills/SKILL.md`. This is intentional and correct: the skill named "skills" lives in a directory named "skills". Slightly redundant visually but unambiguous in practice.

## Anti-Patterns

### Anti-Pattern 1: Resolving env var references at codegen time

**What people do:** Call `std::env::var("BROWSER_USE_API_KEY")` in `generate_wrapper()`, inline the resolved value as a string literal in the generated script.

**Why wrong:** Writes secret values into plaintext wrapper scripts on disk. Also breaks when the host env at `rightclaw up` time differs from the process-compose execution environment.

**Do this instead:** Emit `export KEY="${VALUE}"` verbatim. Bash resolves at runtime in the correct environment.

### Anti-Pattern 2: Wrapping env in a struct with deny_unknown_fields

**What people do:** Create `pub struct EnvConfig(HashMap<String, String>)` and apply `#[serde(deny_unknown_fields)]`.

**Why wrong:** `deny_unknown_fields` is meaningful on structs with named fields. `HashMap` accepts all string keys by definition — the attribute has no effect and adds indirection for no gain.

**Do this instead:** `pub env: HashMap<String, String>` directly on `AgentConfig`. The existing `#[serde(deny_unknown_fields)]` on `AgentConfig` already protects unknown top-level keys.

### Anti-Pattern 3: Naming the skill directory rightskills

**What people do:** Follow SEED-006's "rightskills" naming literally.

**Why wrong:** The install path `.claude/skills/rightskills/` produces slash command `/rightskills` — awkward branding for a generic skill manager. The skill's user-facing name is `/skills`.

**Do this instead:** Install at `.claude/skills/skills/SKILL.md`. Source at `skills/skills/SKILL.md`. Const named `SKILL_SKILLS`.

## Recommended Build Order

### Phase 1: Env var injection (self-contained)

1. Add `env: HashMap<String, String>` to `AgentConfig` in `types.rs`
2. Write failing tests: `agent_config_with_env_vars`, `agent_config_env_defaults_empty`
3. Make tests pass
4. Update `generate_wrapper()` in `shell_wrapper.rs` to pass `env_vars` to template context
5. Update `agent-wrapper.sh.j2` with for-loop export block
6. Write failing wrapper tests: `wrapper_exports_env_vars`, `wrapper_no_env_vars_produces_no_exports`
7. Make tests pass

Rationale: clean vertical slice (data model → codegen → template) with no dependency on rename work.

### Phase 2: `/skills` rename (can run in parallel with Phase 1)

1. `git mv skills/clawhub skills/skills`
2. Update `codegen/skills.rs` const + include_str! + install tuple
3. Update tests in `codegen/skills.rs` and `init.rs` that assert on old path
4. Verify `include_str!` compiles (path must resolve at compile time)

Rationale: pure rename + path string changes + test updates. No logic.

### Phase 3: SKILL.md content update (depends on Phase 2)

1. Edit `skills/skills/SKILL.md` policy gate: add `metadata.requires.*` alias for agentskills.io format
2. Add ClawHub as secondary search fallback section
3. Verify instruction accuracy

Rationale: content-only, no Rust compilation. Blocked only on Phase 2 (the file must exist at the new path).

### Dependency graph

```
Phase 1 (env vars)  ─────────────────────────► done
Phase 2 (rename)    ──────────────────────────► done
                                    |
Phase 3 (SKILL.md)  ─── after P2 ──► done
                                              |
                                   integration test:
                                 rightclaw up with env:
                                 verify wrapper exports
```

## Confidence Assessment

| Area | Confidence | Basis |
|------|------------|-------|
| env field placement in AgentConfig | HIGH | Direct inspection of types.rs; matches telegram_* field pattern |
| Template injection approach | HIGH | Direct inspection of agent-wrapper.sh.j2 and shell_wrapper.rs |
| Rename scope (all affected files) | HIGH | Direct inspection of skills.rs, init.rs, skill directory, test assertions |
| Policy gate SKILL.md content | HIGH | Direct inspection — already CC-native, no policy.yaml refs |
| skills.sh `npx skills` CLI behavior | MEDIUM | SKILL.md already written for it; not independently reverified |

## Sources

- `crates/rightclaw/src/agent/types.rs` — AgentConfig struct inspection
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — generate_wrapper() inspection
- `crates/rightclaw/src/codegen/skills.rs` — install_builtin_skills() inspection
- `crates/rightclaw/src/init.rs` — println! path string inspection
- `templates/agent-wrapper.sh.j2` — template ordering inspection
- `skills/clawhub/SKILL.md` — policy gate and command content inspection
- `.planning/seeds/SEED-005-skills-sh-instead-of-clawhub.md`
- `.planning/seeds/SEED-006-rename-clawhub-to-rightskills.md`
- `.planning/PROJECT.md` — v2.2 milestone requirements

---
*Architecture research for: RightClaw v2.2 Skills Registry*
*Researched: 2026-03-25*
