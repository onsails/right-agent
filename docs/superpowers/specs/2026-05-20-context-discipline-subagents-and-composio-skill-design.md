# Context discipline: subagent rule + bundled Composio skill

**Date:** 2026-05-20
**Status:** approved

## Problem

Audit of all `him` (18) and `right` (13) Telegram sessions (stream NDJSON
under `~/.right/logs/streams/`) showed that subagent (`Agent` tool)
delegation is sporadic and Composio MCP responses pollute the main
context window:

- 7 of 18 `him` sessions used subagents at all (15 calls total); 2 of 13
  for `right` (3 calls). Several long group-chat sessions
  (`f7d5a319`, `2f4a29c9`, `8db11961`) reach 192K–268K
  `cache_read_input_tokens` per turn — at or beyond Sonnet 4.6's 200K
  ceiling, after which Claude Code auto-compacts silently.
- The biggest single contributor to that bloat is
  `mcp__right__composio__COMPOSIO_MULTI_EXECUTE_TOOL` returning
  paginated `_LIST_`/`_SEARCH_`/`_FETCH_` payloads inline. The tool
  already exposes a `sync_response_to_workbench: true` field that
  redirects the response body to Composio's remote workbench — but
  agents almost never set it.
- The current `### Subagents` section in
  `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
  is permissive ("you may use subagents") with no concrete trigger,
  so agents underdelegate.
- Two cheap calls were *over-delegated* in DM sessions
  (`5e4fb4db`, `17c9866d` — both `чекни последние письма gmail`): the
  agent spawned subagents just to run `mcp__right__mcp_list`, paying
  CC subprocess overhead for a small response. So the rule needs an
  explicit "don't delegate when" list too.

## Goal

Land two coupled system-prompt changes that ship to all existing agents
on `right restart`, no sandbox recreation, no per-agent migration:

1. Rewrite `### Subagents` in `OPERATING_INSTRUCTIONS.md` around the
   principle that delegation is appropriate when intermediate results
   don't need to live in the main context.
2. Bundle a new core skill `right-composio` covering
   workbench-vs-context discipline and Composio tool-selection
   patterns. Activation gated by the skill's `description:` frontmatter
   ("when composio MCP is mounted and you're about to call
   `mcp__right__composio__*`").

## Non-goals

- Custom subagent types (`gmail-triage`, `notion-research`, etc.).
  The user explicitly defers this to learned `rightx-*` autoskills.
- Touching `IDENTITY.md` / `SOUL.md` shapes or any per-agent owned
  files. The rule lives in the platform-managed system prompt.
- Token-count thresholds (`>5K`, `<1K`). The principle is
  "intermediate-result relevance," not bytes counted.
- A general-purpose `right-mcp-payload-hygiene` skill covering any
  heavy MCP. Composio is the only concrete offender right now;
  future heavy MCPs get their own skill when they appear.
- Conditional rendering of `## Core Skills` based on installed MCPs.
  Static registration + skill-`description:` gating is sufficient.
- Changes to `mcp_instructions.rs` (that path is for upstream
  `peer_info().instructions`, outside our control).

## Approach

Two artifacts, both `Regenerated(BotRestart)` per ARCHITECTURE.md
"Upgrade & Migration Model":

### A. Subagent rule rewrite

Replace the existing `### Subagents` section (currently ~12 lines,
permissive) in
`crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
with the text below. The principle is: delegate when intermediate
results are not needed in main context; do not delegate when
intermediate output drives the next step or when the call is cheap
and small.

```markdown
### Subagents

Use the built-in Claude Code `Agent` tool when you can offload work
whose intermediate results don't need to live in your main context.
Two canonical triggers:

1. **Multi-step workflows where only the final outcome matters.**
   Researching across several sources, building a candidate list and
   picking from it, comparing options — dispatch the whole loop and
   take back only the conclusion.

2. **File or tool reads where only the verdict matters.**
   "Does this JSONL contain a specific decision?", "Find the endpoint
   URL on this docs page", "Summarize what this long Composio response
   says about X" — read in a subagent, take back the answer.

Do NOT delegate when:
- You need to see the intermediate output to decide the next step in
  the same turn.
- The task is one cheap tool call with a small response (e.g.
  `mcp__right__mcp_list`, a single `mcp__right__cron_trigger`, a
  `mcp__right__send_progress` update).
- The work is a short edit, single command, or quick verification
  whose entire output you'd read anyway.

For independent subtasks (e.g. "research these three options"),
dispatch multiple subagents in one message via parallel `Agent`
tool calls — sequential dispatch wastes time.

The main session is accountable: give the subagent a bounded prompt,
review its output, resolve conflicts with what you already know, and
synthesize for the user. Do not paste raw subagent output as the
final answer.
```

Verbatim text. Tool names use the prefixed `mcp__right__*` form per
AGENTS.md.

### B. Bundled `right-composio` skill

New file at `crates/right-codegen/skills/right-composio/SKILL.md`:

````markdown
---
name: right-composio
description: >-
  Use when the user's request maps to a Composio-fronted service
  (Notion, Gmail, Calendar, Slack, GitHub, etc.) and you're about to
  call mcp__right__composio__*. Covers workbench-vs-context discipline,
  MULTI_EXECUTE batching, and search_tools discovery. Activate ONLY
  when composio is in your MCP list.
---

# /right-composio — Composio MCP playbook

Composio is a gateway: one MCP server fronts 250+ external services
(Notion, Gmail, Calendar, Slack, GitHub, ...). Tool surface is narrow
(~7 meta-tools) but responses can be huge. Two biggest context risks:
dumping list/search/fetch payloads into context, and looping single
tool calls when one MULTI_EXECUTE would do.

## When to Activate

- The user's request maps to a Composio-fronted service.
- You're about to invoke `mcp__right__composio__*` and need to decide:
  workbench yes/no, MULTI_EXECUTE vs single, search_tools first?
- If composio is not in `mcp__right__mcp_list`, this skill does not
  apply — ask the user to `/mcp add composio <url>`.

## Workbench discipline

`mcp__right__composio__COMPOSIO_MULTI_EXECUTE_TOOL` has a
`sync_response_to_workbench` field. `true` → response stored in
Composio's remote workbench, you get a reference. `false` (default)
→ full payload lands in your context.

**`sync_response_to_workbench: true` when:**
- Tool slug contains `_LIST_`, `_SEARCH_`, `_FETCH_`, `_GET_ALL`,
  `_PAGES`, `_THREADS` (collections).
- Batching 2+ tools in one MULTI_EXECUTE call.
- Expecting prose bodies (email content, Notion page text).
- Follow-up MULTI_EXECUTE will act on the result — pass the
  workbench reference via `session_id`.

**`sync_response_to_workbench: false` (or omit) when:**
- Single write/update returning only an id or status
  (`NOTION_INSERT_ROW_DATABASE`, `GMAIL_SEND_EMAIL`,
  `CALENDAR_CREATE_EVENT`).
- Single read of one known record where the body IS the user's
  answer (`NOTION_FETCH_PAGE` by id when the user asked "what's on
  that page").
- Next step in this turn branches on the result AND the result
  is small.

When in doubt: workbench on. Pull with
`mcp__right__composio__COMPOSIO_REMOTE_WORKBENCH` later.

## Tool-selection patterns

- **Unknown toolkit slug?** Always
  `mcp__right__composio__COMPOSIO_SEARCH_TOOLS` first. Don't guess —
  slugs change.
- **Multiple ops on same toolkit?** One MULTI_EXECUTE with a `tools`
  array beats N separate calls.
- **Non-trivial query/transform on a result?**
  `mcp__right__composio__COMPOSIO_REMOTE_BASH_TOOL` on workbench
  data beats pulling-and-parsing in context.

## Pitfalls

- **`input` vs `arguments`:** per-tool args go under `arguments`, not
  `input`. "Required at" / "missing fields" errors = your fault.
- **Connection errors:** `has_active_connection: false` is a
  toolkit-level Composio↔external auth, not MCP-transport auth.
  Call `mcp__right__composio__COMPOSIO_MANAGE_CONNECTIONS` as the
  upstream tells you. Do NOT suggest `/mcp auth composio`. (See
  "MCP Error Diagnosis → Trust upstream diagnostics" in your main
  prompt.)
````

Registered in `crates/right-codegen/src/skills.rs`:

```rust
const SKILL_RIGHT_COMPOSIO: Dir = include_dir!("$CARGO_MANIFEST_DIR/skills/right-composio");

pub const BUILTIN_SKILL_NAMES: &[&str] = &[
    "right-skills",
    "right-cron",
    "right-mcp",
    "right-learn-skill",
    "right-memory",
    "right-reflect",
    "right-composio",  // new
];

fn builtin_skill_dir(name: &str, memory_provider: &MemoryProvider) -> miette::Result<&'static Dir<'static>> {
    match name {
        // ... existing arms ...
        "right-composio" => Ok(&SKILL_RIGHT_COMPOSIO),
        _ => Err(miette::miette!("unknown builtin skill {name:?} ...")),
    }
}
```

`crates/right-platform-store/src/lib.rs::build_manifest` already
iterates `BUILTIN_SKILL_NAMES` (single source of truth — see comment
on `BUILTIN_SKILL_NAMES`), so the sandbox deployer picks up
`right-composio` automatically.

`crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
gets one new line under `## Core Skills`:

```markdown
- `/right-composio` — playbook for Composio MCP. Use when calling
  `mcp__right__composio__*` and composio is in your MCP list.
```

`PROMPT_SYSTEM.md` gets a one-line update where Core Skills are
enumerated, mirroring the new entry.

## Architecture

No new runtime, no new MCP tools, no new SQL tables. All changes are
content edits to platform-managed files plus a one-skill addition to
`BUILTIN_SKILL_NAMES`. Categories per ARCHITECTURE.md:

| File | Change | Category |
|---|---|---|
| `templates/right/prompt/OPERATING_INSTRUCTIONS.md` | rewrite `### Subagents`, add `/right-composio` line | `Regenerated(BotRestart)` |
| `skills/right-composio/SKILL.md` | new file, ~75 lines | `Regenerated(BotRestart)` |
| `src/skills.rs` | const + name + match arm | code |
| `PROMPT_SYSTEM.md` | mention `/right-composio` in Core Skills list | doc |

`Regenerated(BotRestart)` means: on `right restart <agent>`, the
codegen pipeline rewrites the file under
`agents/<name>/.claude/skills/right-composio/SKILL.md` and refreshes
the rendered system prompt. The platform store syncs the skill into
`/sandbox/.claude/skills/` on the next claude invocation. No sandbox
recreation, no `agent config` step, no Telegram-managed migration.

Activation of `right-composio` happens entirely via Claude Code's
skill-selector matching on the `description:` field. The Right Agent
codebase has no runtime decision about when the skill loads — that's
delegated to CC.

## Data flow

For a typical post-merge user turn that triggers both changes:

1. User asks `him` "глянь последние письма gmail".
2. Composite system prompt is assembled (per PROMPT_SYSTEM.md). It
   contains the rewritten `### Subagents` section and the
   `/right-composio` Core Skills entry.
3. CC's skill selector sees the description match (composio MCP is
   mounted + user request maps to Gmail) and loads
   `right-composio/SKILL.md` into the turn.
4. Agent applies skill guidance:
   - Calls `mcp__right__composio__COMPOSIO_MULTI_EXECUTE_TOOL` with
     `sync_response_to_workbench: true` (because the operation is a
     `_LIST_`/`_FETCH_` returning prose bodies).
   - Pulls a summary from the workbench reference rather than the
     raw payload.
5. Final answer to the user does not include the raw Gmail JSON.

For an over-delegation case (`чекни MCP список`):

1. User asks "проверь MCP список".
2. Agent sees the rewritten subagent rule's negative list
   (`mcp__right__mcp_list` is an explicit "do NOT delegate"
   example).
3. Agent calls `mcp__right__mcp_list` directly inline, reads the
   small response, answers.

## Error handling

Skill files are best-effort content: if CC's skill selector fails to
load `right-composio` for some reason, the agent falls back to the
generic Composio behavior described in `OPERATING_INSTRUCTIONS.md`
under "MCP Error Diagnosis" (already covers the auth-disambiguation
case) — no regression.

If `install_builtin_skills` fails to write
`agents/<name>/.claude/skills/right-composio/SKILL.md` (disk full,
permissions), the existing `miette::Result` propagation surfaces the
error at codegen time; the bot will not start the affected agent.
That's already the contract — no new error paths.

## Testing

### Automated (rust)

Add to `crates/right-codegen/src/skills_tests.rs`:

- `right_composio_in_builtin_skill_names` — asserts
  `BUILTIN_SKILL_NAMES.contains(&"right-composio")`.
- `right_composio_resolves_to_dir` — asserts
  `builtin_skill_dir("right-composio", &MemoryProvider::Hindsight)`
  returns `Ok` and the dir has at least one file ending in
  `SKILL.md`.

`crates/right-platform-store/src/platform_store_tests.rs::build_manifest_deploys_all_listed_builtin_skills`
already iterates `BUILTIN_SKILL_NAMES`; no changes there.

No assertion is added for `OPERATING_INSTRUCTIONS.md` text content
(no existing renderer-tests for this template; not introducing a
fixture purely for this change).

### Verification cadence

Per AGENTS.md verification rules:

- TDD loop: write the two skills_tests first, watch them fail
  (right-composio not in list, dir missing), then add the const +
  name + match arm + SKILL.md, watch them pass.
- Targeted check after implementation: `devenv shell -- cargo test
  -p right-codegen`.
- Final mandatory: `devenv shell -- cargo test --workspace` from the
  worktree before declaring complete.

### Manual verification (post-merge, on operator machine)

1. `right restart him && right restart right`.
2. `ls ~/.right/agents/him/.claude/skills/right-composio/` shows
   `SKILL.md`.
3. In Telegram, send `him` a Composio-Gmail task ("глянь последние
   письма"). In `~/.right/logs/streams/<latest-sid>.ndjson`:
   - Look for a Skill invocation referencing `right-composio` (CC
     surfaces this as a `<command-name>` system event or a tool_use
     with name `Skill` and the skill slug).
   - Confirm the next `COMPOSIO_MULTI_EXECUTE_TOOL` call has
     `"sync_response_to_workbench":true` in its arguments.
4. Send "проверь MCP список" — confirm no `tool_use.name=="Agent"`
   appears that turn (no over-delegation).
5. Send a research task across 3 sources — confirm multiple
   `tool_use.name=="Agent"` blocks in the *same* assistant message
   (parallel dispatch).

## Open questions (post-deploy observations)

- If activation of `right-composio` proves unreliable under CC's
  current selector heuristics, the fallback is a hint inside
  `peer_info().instructions` upstream of Composio — out of our
  control. We'd then bring the skill content inline into
  `OPERATING_INSTRUCTIONS.md` under a "Composio quick reference"
  subsection. Observable signal: `sync_response_to_workbench: true`
  rate on `_LIST_` calls.
- If the principle-based subagent rule still produces over-delegation
  (`mcp_list` spawned in subagents), tighten the negative list with
  more explicit tool-name patterns. Observable signal: subagent
  count for sessions with only `mcp_list`-shaped intent.
- Neither of these is a blocker for shipping the current change.

## References

- Earlier audit results (from this conversation): subagent call counts
  per session, max `cache_read_input_tokens` per session, and the
  short list of sessions that overdelegated cheap tool calls.
- `docs/superpowers/specs/2026-05-06-composio-auth-disambiguation-design.md`
  — the existing vendor-agnostic MCP error-diagnosis fix; this skill
  defers to it rather than duplicating.
- `ARCHITECTURE.md` → "Upgrade & Migration Model" (categories table)
  and "Codegen ownership rules" (single-source-of-truth for
  `BUILTIN_SKILL_NAMES`).
- `PROMPT_SYSTEM.md` → composite system prompt assembly.
