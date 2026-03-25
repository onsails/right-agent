# Stack Research: v2.2 Skills Registry

**Domain:** Multi-agent runtime CLI — Skills Registry Integration (v2.2)
**Researched:** 2026-03-25
**Confidence:** HIGH

## Scope

Delta-research for the v2.2 Skills Registry milestone. Covers ONLY what is new or changed relative to the validated v2.1 stack. The v2.1 validated stack (reqwest, serde, serde-saphyr, minijinja, tokio, etc.) is not re-evaluated.

---

## Verdict: No New Rust Crates Required

Every capability in v2.2 maps to existing workspace dependencies. The analysis below shows the mapping for each new feature.

---

## Capability Analysis

### 1. skills.sh Search API

**What we need:** HTTP call to the skills.sh registry for the `/skills search` command.

**API details (verified from CLI source `src/find.ts` in `vercel-labs/skills`):**
- Endpoint: `GET https://skills.sh/api/search?q={encoded_query}&limit=10`
- Auth: none — fully unauthenticated, no API key, no token, no headers
- Response: JSON array of skill objects
- The CLI also supports `SKILLS_API_URL` env var to override the base URL (irrelevant for us)

**Context:** The `/skills` SKILL.md already instructs the Claude Code agent to call `npx skills find <query>` first, with a GitHub topic search fallback. The Rust side does NOT need to implement search — it's instruction-driven agent behavior. If a future Rust-side registry query is added, `reqwest` + `serde_json` (both already in workspace) are sufficient.

**New crates:** None.

**Confidence:** HIGH — verified from CLI source code at `github.com/vercel-labs/skills`

---

### 2. skills.sh Install Mechanism

**What we need:** Install a skill by `owner/repo` slug.

**How install works:**
- There is NO HTTP install API. Installation is entirely git-based: `git clone https://github.com/owner/repo`, then copy SKILL.md into `.claude/skills/<name>/`.
- `npx skills add owner/repo` is the canonical install path.
- The `/skills` SKILL.md (already written at `skills/clawhub/SKILL.md`, to be renamed) delegates install to `npx skills` CLI with a `git clone` fallback — this all happens inside the Claude Code agent session, not in Rust.

**Rust involvement:** None. The orchestrator does not need to implement skill installation.

**New crates:** None.

**Confidence:** HIGH — verified from CLI README and source structure

---

### 3. env: Section in agent.yaml

**What we need:** Per-agent environment variable injection:
```yaml
env:
  BROWSER_USE_API_KEY: "${BROWSER_USE_API_KEY}"  # expand from host env at launch time
  CUSTOM_VAR: "literal-value"
```

**Design:**
- `AgentConfig` gains `pub env: HashMap<String, String>` with `#[serde(default)]` in `crates/rightclaw/src/agent/types.rs`.
- At `rightclaw up` time, values matching `${VAR_NAME}` pattern are expanded via `std::env::var()` (stdlib). Literal values pass through unchanged. This is ~10 lines of stdlib Rust, no crate needed.
- The `agent-wrapper.sh.j2` template gains an `export KEY="VALUE"` block before `exec "$CLAUDE_BIN"`, rendered via minijinja (already in workspace).
- `deny_unknown_fields` on `AgentConfig` means the field must be added to the struct or existing agent.yaml files with `env:` will fail to parse — add the field.

**Existing stack coverage:**
| Need | Covered By |
|------|-----------|
| Deserialize `env:` map | `serde` + `HashMap<String, String>` + `serde-saphyr` (all in workspace) |
| Expand `${VAR}` at launch | `std::env::var()` — stdlib |
| Inject into wrapper script | `minijinja` template variable (already used for other conditional blocks) |

**New crates:** None. `envy` crate is NOT appropriate here — it reads env vars into Rust structs (wrong direction). `shellexpand` crate is overkill for a one-pattern substitution.

**Confidence:** HIGH

---

### 4. Policy Gate Rework

**What we need:** Drop `metadata.openclaw.requires` / OpenShell policy.yaml checks from the policy gate. Replace with CC-native sandbox capability checks (reading agent's `.claude/settings.json`).

**Where this lives:** Entirely in `skills/clawhub/SKILL.md` (instruction text for the CC agent). The current SKILL.md already contains updated policy gate logic checking `settings.json` sandbox sections (see lines 99-121 of the existing file). No Rust code change needed.

**New crates:** None.

**Confidence:** HIGH

---

### 5. Skill Rename: clawhub → skills

**What we need:** Rename `SKILL_CLAWHUB` const and install path from `clawhub` to `skills`.

**Files affected:**
- `crates/rightclaw/src/init.rs` — `SKILL_CLAWHUB` constant
- `crates/rightclaw/src/codegen.rs` — skill install path
- `skills/clawhub/` directory — rename to `skills/skills/`

**New crates:** None. String constant changes only.

**Confidence:** HIGH

---

## Existing Dependencies (Unchanged, Relevant to v2.2)

| Crate | Version | How Used in v2.2 |
|-------|---------|-----------------|
| reqwest | 0.13 | Available for skills.sh `GET /api/search` if Rust-side search is added later |
| serde | 1.0 | `HashMap<String, String>` for `env:` field deserialization |
| serde-saphyr | 0.0 | Deserializes updated `AgentConfig` with new `env:` field |
| serde_json | 1.0 | Deserialize skills.sh JSON response if Rust-side search is added |
| minijinja | 2.18 | Extend `agent-wrapper.sh.j2` with per-agent env var exports |
| tokio | 1.50 | Async runtime for any future reqwest calls |

---

## What NOT to Add

| Avoid | Why |
|-------|-----|
| `envy` | Wrong problem — it maps host env vars INTO a Rust struct; we export YAML-specified vars TO a shell |
| `shellexpand` | Adds a dependency for a 5-line stdlib pattern match on `${VAR_NAME}` |
| `serde_yml` or `serde_yaml` | Already have `serde-saphyr`; these are deprecated or less rigorously tested forks |
| Any new HTTP client | `reqwest` already in workspace; sufficient for unauthenticated GET |
| Rust-side git clone for skill install | Agent-side via `npx skills`; Rust orchestrator has no role in skill installation |

---

## Integration Points Summary

| Feature | Changed Files | New Crates |
|---------|--------------|------------|
| Skill rename | `init.rs`, `codegen.rs`, `skills/clawhub/` → `skills/skills/` | None |
| `env:` in agent.yaml | `agent/types.rs` (add field), `agent-wrapper.sh.j2` (add exports block), `upcommand.rs` or codegen (expand `${VAR}`) | None |
| Policy gate rework | `skills/skills/SKILL.md` (instruction text only) | None |
| skills.sh search (agent-side) | `skills/skills/SKILL.md` already done | None |

---

## Sources

- `https://raw.githubusercontent.com/vercel-labs/skills/main/src/find.ts` — verified search endpoint `https://skills.sh/api/search?q=&limit=10`, no auth required (2026-03-25)
- `https://github.com/vercel-labs/skills` — CLI architecture, GitHub-native install (no install HTTP API) (2026-03-25)
- `https://skills.sh/docs` — no REST API documented beyond CLI (2026-03-25)
- `https://serde.rs/` — `HashMap<String, String>` deserialization, HIGH confidence
- Existing codebase: `Cargo.toml`, `crates/rightclaw/src/agent/types.rs`, `templates/agent-wrapper.sh.j2` — confirmed existing stack coverage

---
*Stack research for: RightClaw v2.2 Skills Registry milestone*
*Researched: 2026-03-25*
