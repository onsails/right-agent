# ARCHITECTURE.md Trim — Design

## Goal

Shrink `ARCHITECTURE.md` from ~42.6k chars (755 lines) to ~33–34k chars (~20% reduction) by cutting derivable content (file trees, schema dumps, type catalogs) and lightly tightening procedural narratives, while preserving all load-bearing rules and conventions verbatim. Fix stale content discovered during the pass.

**Primary motivation:** the file is included via `@ARCHITECTURE.md` in `CLAUDE.md`, so every character costs tokens on every conversation turn. Removing content that's either derivable (file trees) or rot-prone (schema dumps) reduces per-turn cost without sacrificing the rules that can't be rediscovered from code.

**Non-goal:** splitting into satellite docs. Satellite reference docs rot faster than inline ones because they're not in the always-loaded context — and this project's "self-healing platform" ethos depends on the doc staying true.

## Scope

**In scope:**
- In-place edits to `/Users/molt/dev/rightclaw/ARCHITECTURE.md`.
- Correction of stale content spotted during the trim pass.

**Out of scope:**
- `CLAUDE.md`, `CLAUDE.rust.md`, `PROMPT_SYSTEM.md` (separate docs).
- Restructuring section ordering.
- Adding new content.
- Any code change.

## Section-by-Section Plan

### Cut hard (files, layouts, schemas, types)

| Section | Current | Target | Approach |
|---|---|---|---|
| Module Map — `rightclaw` (core) | ~45 lines, full file tree with inline comments | ~8 lines | One line per submodule (`agent/`, `config/`, `codegen/`, `memory/`, `runtime/`, `mcp/`) stating purpose. Top-level single-module files listed as a single line. Pointers only where load-bearing (e.g. `codegen/contract.rs` is referenced by Upgrade Model). |
| Module Map — `rightclaw-cli` | ~10 lines, file tree | ~2 lines | Module names with one-line purpose each. |
| Module Map — `rightclaw-bot` | ~28 lines, file tree | ~5 lines | `telegram/`, `login.rs`, `sync.rs`, `cron.rs`, `cron_delivery.rs`, `stt/` — one line each. |
| Key Types | 12 lines, type list | 0 lines | Delete. Names appear in narrative sections where they matter. |
| Memory Schema (SQLite) | 13 lines, column-level table definitions | 2 lines | Names-only table list. `sqlite3 data.db .schema` is authoritative. |
| Directory Layout (Runtime) | 40 lines, full tree | ~10 lines | Keep a trimmed tree showing only critical paths that function as invariants: `~/.rightclaw/config.yaml`, `agents/<name>/{agent.yaml,data.db,policy.yaml}`, `agents/<name>/.claude/.credentials.json` symlink, `run/{process-compose.yaml,state.json,internal.sock}`, `backups/<agent>/`, `logs/`. Drop the rest. |

**Expected savings from hard cuts:** ~7k chars.

### Keep + light prose tightening (procedural narratives)

Sections preserved structurally. Light-touch edits only: remove sentences that restate themselves, tighten inline parenthetical comments, fix redundant phrasing. Target ~5–10% shrink per section, not 50%.

- Agent Lifecycle
- Voice transcription
- OpenShell Sandbox Architecture
- Login Flow (setup-token)
- MCP Token Refresh
- MCP Aggregator
- Prompting Architecture
- Claude Invocation Contract
- Reflection Primitive
- Stream Logging
- Memory
- Memory Resilience Layer

**Expected savings from tightening:** ~1.5–2k chars.

### Keep verbatim (load-bearing rules, gotchas, tables)

No edits except mechanical stale-data fixes caught in the stale-check pass.

- Workspace (top of doc)
- MCP Auth Types (table)
- Configuration Hierarchy (table)
- External Integrations (table)
- Runtime isolation — mandatory (full section including PC_API_TOKEN subsection)
- SQLite Rules
- Upgrade & Migration Model (all subsections)
- Integration Tests Using Live Sandboxes
- Security Model
- OpenShell Integration Conventions
- OpenShell Policy Gotchas
- Logging

## Stale-Check Protocol

While editing, cross-reference content against code and flag contradictions **inline in the PR/commit** before "fixing" them. Do not silently rewrite.

**Method:** for each narrative section being tightened, open the corresponding code and verify:

- Module paths and file names still exist.
- Port numbers (`18927`, `8100`, `8080`, `3128`) still match.
- Env var names still match (`HTTPS_PROXY`, `CLAUDE_CODE_OAUTH_TOKEN`, `PC_API_TOKEN`).
- Socket paths still match (`~/.rightclaw/run/internal.sock`, `oauth-callback.sock`).
- Table names in Memory Schema still exist (`memories`, `memory_events`, `telegram_sessions`, etc.).
- Tool counts (`13 built-in tools`, etc.) still match.
- "Deprecated" markers — is the feature actually gone, or still active?

If a contradiction is found, list it at the top of the commit message as `stale-fixes:` with the correction, then apply the fix alongside the trim edit.

## Success Criteria

- **Size:** final ARCHITECTURE.md is ≤ 34k chars. Hard ceiling: 36k.
- **Content preservation:** every rule in "Keep verbatim" is present unchanged (aside from mechanical stale-data fixes).
- **No broken cross-references:** sections referenced elsewhere in the file (e.g. `see [Upgrade & Migration Model]`) still resolve.
- **No external broken refs:** `CLAUDE.md`, `PROMPT_SYSTEM.md`, and other docs that link to ARCHITECTURE.md sections still resolve. Grep for `ARCHITECTURE.md#` across the repo before committing.
- **Readable:** a reader unfamiliar with the repo can still orient within 5 minutes using only this file. Smoke test: can a new reader find where MCP token refresh lives after the trim? Yes → the pointer is sufficient; No → pointer needs a line added.

## Risks & Mitigations

- **Over-trimming a narrative that turns out to be load-bearing.** Mitigation: "narrative" sections get ≤10% shrink, so structural content survives. If a section feels like it needs >10% cuts, flag before editing.
- **Silent removal of content referenced elsewhere.** Mitigation: grep for `ARCHITECTURE.md#<anchor>` before deleting any section or renaming any heading.
- **Stale-check produces surprises that expand scope.** Mitigation: limit stale-fixes to one-line corrections. If a stale finding requires a multi-line rewrite or genuine architectural work, record it as a TODO in commit message and handle separately.

## Rollout

Single commit: `docs: trim ARCHITECTURE.md (~42k → ~33k chars)`. Body lists hard-cut sections and any `stale-fixes:`. No code changes, no migrations, no follow-up.
