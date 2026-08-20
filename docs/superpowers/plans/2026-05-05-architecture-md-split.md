# ARCHITECTURE.md split: prescriptive vs descriptive — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `ARCHITECTURE.md` from 42 KB into ~14–16 KB prescriptive core plus six descriptive satellite files under `docs/architecture/*.md`. Drop ~8 KB of derivable dependency rationale from `CLAUDE.md` and add a "cite-on-touch" convention so the satellites stay maintained.

**Architecture:** Three atomic commits. Commit 1 creates satellite files (content briefly duplicated). Commit 2 trims `ARCHITECTURE.md` (deletes duplicates, applies tighten edits, leaves plain-path pointers). Commit 3 cleans up `CLAUDE.md` and adds the new convention.

**Tech Stack:** Markdown only. No code, no tests in the unit-test sense. Verifications use `wc -c`, `rg`, and direct file inspection.

**Spec:** [`docs/superpowers/specs/2026-05-05-architecture-md-split-design.md`](../specs/2026-05-05-architecture-md-split-design.md)

**Working directory:** This plan does docs-only edits on `master`. A worktree is not required (low blast radius, no code, no CI dependency). If you prefer isolation, run `git worktree add .worktrees/arch-split` first and execute there.

---

## File Structure

**New files** (created in Task 1, modified later only via the cite-on-touch rule):

| File | Owner content | Approx size |
|---|---|---|
| `docs/architecture/modules.md` | Crate/module breakdown (formerly `## Module Map`) | ~3 KB |
| `docs/architecture/lifecycle.md` | Agent lifecycle pseudocode + voice transcription + login flow | ~7 KB |
| `docs/architecture/sandbox.md` | OpenShell sandbox staging dir, platform store, TLS-MITM, network | ~2 KB |
| `docs/architecture/mcp.md` | MCP token refresh + aggregator dispatch detail | ~2 KB |
| `docs/architecture/memory.md` | Memory modes (Hindsight/file) + auto-retain/recall + resilience layer | ~5 KB |
| `docs/architecture/sessions.md` | Stream logging + per-session mutex + reflection limits + cron schedule variants | ~5 KB |

**Modified files:**

- `ARCHITECTURE.md` — net shrink from 42 KB → ~14–16 KB.
- `CLAUDE.md` — net shrink from 13 KB → ~5 KB; new "Architecture docs split" section added.

**Untouched:** `CLAUDE.rust.md`, `PROMPT_SYSTEM.md`, all source code, all other docs.

**Each satellite file starts with this status header (literal, including the leading `>`):**

```markdown
> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `CLAUDE.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.
```

---

## Task 1: Create satellite docs (Commit 1)

This task adds new files only. `ARCHITECTURE.md` is not edited — content is briefly duplicated. That makes the diff trivially reviewable.

**Files:**
- Create: `docs/architecture/modules.md`
- Create: `docs/architecture/lifecycle.md`
- Create: `docs/architecture/sandbox.md`
- Create: `docs/architecture/mcp.md`
- Create: `docs/architecture/memory.md`
- Create: `docs/architecture/sessions.md`

### Step 1.1: Create `docs/architecture/` directory

- [ ] Run:

```bash
mkdir -p /Users/molt/dev/rightclaw/docs/architecture
```

### Step 1.2: Create `modules.md`

- [ ] Read `ARCHITECTURE.md` lines 13–42 (the `## Module Map` section through end of `### right-bot` subsection).
- [ ] Write `/Users/molt/dev/rightclaw/docs/architecture/modules.md` with this structure:
  - Line 1: `# Modules`
  - Line 2: blank
  - Lines 3–5: the status-header blockquote (verbatim, see "File Structure" above).
  - Line 6: blank
  - Lines 7+: the original `## Module Map` content from `ARCHITECTURE.md` lines 13–42, demoted by one heading level (`## Module Map` → `## Module Map` is fine; `### right-agent (core)` → `### right-agent (core)` stays). No edits to body text.

The result is a self-contained file that preserves the original section verbatim under a top-level title.

### Step 1.3: Create `lifecycle.md`

- [ ] Write `/Users/molt/dev/rightclaw/docs/architecture/lifecycle.md` containing, in order:
  - Title: `# Lifecycle and runtime flows`
  - blank line
  - status-header blockquote (verbatim)
  - blank line
  - The full content of `ARCHITECTURE.md` lines 46–152 verbatim (`### Agent Lifecycle` through end of its body), but rename the section `### Agent Lifecycle` → `## Agent Lifecycle` (promote one level since this satellite's H1 is the file title).
  - blank line
  - The full content of `ARCHITECTURE.md` lines 153–172 verbatim, with `### Voice transcription` → `## Voice transcription`.
  - blank line
  - The full content of `ARCHITECTURE.md` lines 210–230 verbatim, with `### Login Flow (setup-token)` → `## Login Flow (setup-token)`.

### Step 1.4: Create `sandbox.md`

- [ ] Write `/Users/molt/dev/rightclaw/docs/architecture/sandbox.md` containing:
  - `# OpenShell sandbox`
  - blank line
  - status-header blockquote
  - blank line
  - The full content of `ARCHITECTURE.md` lines 173–209 verbatim (`### OpenShell Sandbox Architecture` through end of its code block), with `### OpenShell Sandbox Architecture` → `## OpenShell Sandbox Architecture`.

(Yes, this duplicates the "sandboxes are persistent" line that will also remain in the trimmed `ARCHITECTURE.md`. Slight duplication is fine; the satellite is the authoritative descriptive doc.)

### Step 1.5: Create `mcp.md`

- [ ] Write `/Users/molt/dev/rightclaw/docs/architecture/mcp.md` containing:
  - `# MCP Aggregator and token refresh`
  - blank line
  - status-header blockquote
  - blank line
  - The full content of `ARCHITECTURE.md` lines 231–243 verbatim (`### MCP Token Refresh`), promoted to `## MCP Token Refresh`.
  - blank line
  - The full content of `ARCHITECTURE.md` lines 255–274 verbatim (`### MCP Aggregator`), promoted to `## MCP Aggregator`.

### Step 1.6: Create `memory.md`

- [ ] Write `/Users/molt/dev/rightclaw/docs/architecture/memory.md` containing:
  - `# Memory subsystem`
  - blank line
  - status-header blockquote
  - blank line
  - Full content of `ARCHITECTURE.md` lines 421–463 (`### Memory`), promoted to `## Memory`.
  - blank line
  - Full content of `ARCHITECTURE.md` lines 464–484 (`### Memory Resilience Layer`), promoted to `## Memory Resilience Layer`.

### Step 1.7: Create `sessions.md`

- [ ] Write `/Users/molt/dev/rightclaw/docs/architecture/sessions.md` containing:
  - `# Sessions, streams, reflection, cron schedules`
  - blank line
  - status-header blockquote
  - blank line
  - Full content of `ARCHITECTURE.md` lines 337–352 (`### Stream Logging`), promoted to `## Stream Logging`.
  - blank line
  - Full content of `ARCHITECTURE.md` lines 371–390 (`### Per-session mutex on --resume`), promoted to `## Per-session mutex on --resume`.
  - blank line
  - Full content of `ARCHITECTURE.md` lines 313–336 (`### Reflection Primitive`), promoted to `## Reflection Primitive`.
  - blank line
  - Full content of `ARCHITECTURE.md` lines 353–370 (`### Cron Schedule Kinds`), promoted to `## Cron Schedule Kinds`.

### Step 1.8: Verify satellites exist and have content

- [ ] Run:

```bash
ls -la /Users/molt/dev/rightclaw/docs/architecture/
wc -c /Users/molt/dev/rightclaw/docs/architecture/*.md
```

Expected output: six `.md` files, with sizes roughly: `modules.md` ~3 KB, `lifecycle.md` ~7 KB, `sandbox.md` ~2 KB, `mcp.md` ~2 KB, `memory.md` ~5 KB, `sessions.md` ~5 KB. ±20 % is fine — the targets are guidance, not contracts.

### Step 1.9: Verify no `@`-imports inside satellites

- [ ] Run:

```bash
rg '^@' /Users/molt/dev/rightclaw/docs/architecture/ || echo "OK: no @-imports"
```

Expected: `OK: no @-imports`. (The `^@` pattern catches `@`-import lines specifically; mid-line `@` mentions in prose are fine.)

### Step 1.10: Verify status header is present in every file

- [ ] Run:

```bash
for f in /Users/molt/dev/rightclaw/docs/architecture/*.md; do
  if ! grep -q "Status:.*descriptive doc" "$f"; then
    echo "MISSING status header: $f"
  fi
done
echo "Status check done."
```

Expected: only `Status check done.` printed (no `MISSING` lines).

### Step 1.11: Commit

- [ ] Run:

```bash
cd /Users/molt/dev/rightclaw
git add docs/architecture/
git status
```

Expected: six new files staged, nothing else.

- [ ] Commit:

```bash
git commit -m "$(cat <<'EOF'
docs(arch): add docs/architecture/ satellite tree

Six descriptive satellite files extracted from ARCHITECTURE.md. Content
briefly duplicated; ARCHITECTURE.md is trimmed in the next commit.

See docs/superpowers/specs/2026-05-05-architecture-md-split-design.md.
EOF
)"
```

---

## Task 2: Trim `ARCHITECTURE.md` (Commit 2)

This task deletes `MOVE` sections, replaces `TIGHTEN` sections with shorter prescriptive versions, and adds plain-path pointers where deletions happened. After this commit there is no duplicated content.

**Files:**
- Modify: `/Users/molt/dev/rightclaw/ARCHITECTURE.md`

### Step 2.1: Delete `## Module Map` and replace with pointer

- [ ] In `ARCHITECTURE.md`, replace lines 13–42 (the entire `## Module Map` section through the end of `### right-bot`) with exactly this:

```markdown
## Module Map

See: `docs/architecture/modules.md`.
```

### Step 2.2: Delete `### Agent Lifecycle` and replace with pointer

- [ ] Replace lines 46–152 (entire `### Agent Lifecycle` section through the end of the closing code fence) with exactly this:

```markdown
### Agent Lifecycle

See: `docs/architecture/lifecycle.md` (covers `right init`, `right up`,
per-message flow, sandbox migration, `right agent backup`,
`right agent rebootstrap`, `right agent init --from-backup`, and
`right down`).
```

### Step 2.3: Delete `### Voice transcription` and replace with pointer

- [ ] Replace lines 153–172 (entire `### Voice transcription` section) with exactly this:

```markdown
### Voice transcription

See: `docs/architecture/lifecycle.md` (Voice transcription).
```

### Step 2.4: Tighten `### OpenShell Sandbox Architecture`

- [ ] Replace lines 173–209 (entire section including the code-fenced block) with exactly this:

```markdown
### OpenShell Sandbox Architecture

Sandboxes are **persistent** — never deleted automatically. They live as
long as the agent lives and survive bot restarts.

Policy hot-reload via `openshell policy set --wait` covers the network
section only. Filesystem/landlock changes require sandbox recreation
(see `Upgrade & Migration Model` below).

See: `docs/architecture/sandbox.md` for staging-dir layout, platform-store
deployment, TLS-MITM, and the bot-startup sandbox sequence.
```

### Step 2.5: Delete `### Login Flow (setup-token)` and replace with pointer

- [ ] Replace lines 210–230 (entire section) with exactly this:

```markdown
### Login Flow (setup-token)

See: `docs/architecture/lifecycle.md` (Login Flow).
```

### Step 2.6: Delete `### MCP Token Refresh` and replace with pointer

- [ ] Replace lines 231–243 (entire section) with exactly this:

```markdown
### MCP Token Refresh

See: `docs/architecture/mcp.md` (MCP Token Refresh).
```

### Step 2.7: Tighten `### MCP Aggregator`

- [ ] Replace lines 255–274 (entire section) with exactly this:

```markdown
### MCP Aggregator

One shared aggregator process serves all agents on TCP `:8100/mcp` with
per-agent Bearer-token auth. Tool routing rules:

- No `__` prefix → `RightBackend` (built-in tools, unprefixed).
- `rightmeta__` prefix → Aggregator management (read-only: `mcp_list`).
- `{server}__` prefix → `ProxyBackend` (forwarded to upstream MCP).

Internal REST API on Unix socket (`~/.right/run/internal.sock`):
`POST /mcp-add`, `POST /mcp-remove`, `POST /set-token`, `POST /mcp-list`,
`POST /mcp-instructions`. Telegram bot uses `InternalClient` (hyper UDS).
Agents cannot reach the Unix socket from inside the sandbox.

See: `docs/architecture/mcp.md` for dispatch detail and rationale.
```

### Step 2.8: Tighten `### Prompting Architecture`

- [ ] Replace lines 275–289 (entire section) with exactly this:

```markdown
### Prompting Architecture

Every `claude -p` invocation gets a composite system prompt via
`--system-prompt-file` (the sole prompt mechanism — no `--agent` flag).
Prompt caching is critical — avoid per-message tool calls to read
identity files.

See `PROMPT_SYSTEM.md` for full documentation.
```

### Step 2.9: Tighten `### Reflection Primitive`

- [ ] Replace lines 313–336 (entire section) with exactly this:

```markdown
### Reflection Primitive

`crates/bot/src/reflection.rs` exposes
`reflect_on_failure(ctx) -> Result<String, ReflectionError>`. On CC
invocation failure the worker (`telegram::worker`) and cron (`cron.rs`)
call it to give the agent a short `--resume`-d turn wrapped in
`⟨⟨SYSTEM_NOTICE⟩⟩ … ⟨⟨/SYSTEM_NOTICE⟩⟩`, so the agent produces a
human-friendly summary of the failure.

Reflection never reflects on itself. Hindsight `memory_retain` is skipped
for reflection turns. `cron_runs.status` gates delivery: `'failed'` routes
to `DELIVERY_INSTRUCTION_FAILURE`; any other status routes to
`DELIVERY_INSTRUCTION_SUCCESS` (verbatim relay).

See: `docs/architecture/sessions.md` for `ReflectionLimits` (worker vs
cron), usage-event accounting, and label-routing detail.
```

### Step 2.10: Delete `### Stream Logging` and replace with pointer

- [ ] Replace lines 337–352 (entire section) with exactly this:

```markdown
### Stream Logging

See: `docs/architecture/sessions.md` (Stream Logging).
```

### Step 2.11: Tighten `### Cron Schedule Kinds`

- [ ] Replace lines 353–370 (entire section) with exactly this:

```markdown
### Cron Schedule Kinds

`cron_specs.schedule` stores a schedule string that maps to a
`ScheduleKind` variant. The **`Immediate`** variant (encoded as
`schedule = '@immediate'`) is bot-internal — used for
background-continuation jobs and fired on the next reconcile tick (≤5s).
`insert_immediate_cron` defaults `lock_ttl` to
`IMMEDIATE_DEFAULT_LOCK_TTL` (`"6h"`); the lock heartbeat is written once
at job start and never refreshed, so a tighter TTL would let the
reconciler spawn a duplicate `execute_job` against the same spec on the
next 5-second tick. The TTL is the duplicate-prevention guard, not a
wall-clock execution limit.

See: `docs/architecture/sessions.md` for the full variant list.
```

### Step 2.12: Delete `### Per-session mutex on --resume` and replace with pointer

- [ ] Replace lines 371–390 (entire section) with exactly this:

```markdown
### Per-session mutex on --resume

See: `docs/architecture/sessions.md` (Per-session mutex on --resume).
```

### Step 2.13: Delete `### Memory` and replace with pointer

- [ ] Replace lines 421–463 (entire `### Memory` section) with exactly this:

```markdown
### Memory

Two modes, configured per-agent via `memory.provider` in `agent.yaml`:
**Hindsight** (primary, Hindsight Cloud API) and **file** (fallback,
agent-managed `MEMORY.md`). MCP tools `memory_retain` / `memory_recall` /
`memory_reflect` are exposed only in Hindsight mode.

See: `docs/architecture/memory.md` for auto-retain/recall semantics,
prefetch cache behavior, cron-skip rules, and backgrounded-turn handling.
```

### Step 2.14: Delete `### Memory Resilience Layer` and replace with pointer

- [ ] Replace lines 464–484 (entire section) with exactly this:

```markdown
### Memory Resilience Layer

See: `docs/architecture/memory.md` (Memory Resilience Layer).
```

### Step 2.15: Tighten `### PC_API_TOKEN authentication`

- [ ] Replace lines 523–543 (entire section) with exactly this:

```markdown
### PC_API_TOKEN authentication

`right up` generates a random API token (`pc_api_token` in `state.json`)
and passes it to process-compose via `PC_API_TOKEN` env var. PcClient
includes it in every request as the `X-PC-Token-Key` header
(process-compose's only supported scheme — does NOT honor
`Authorization: Bearer`).

**When adding new CLI commands that touch PC, never import `PC_PORT`
directly — always resolve through `from_home(home)`.** For "is PC
running?" probes, treat `Ok(None)` as "no — skip or fail with a clear
message pointing at `right up`". `PC_PORT` may still be referenced by
`cmd_up` (passing `--port` to launch PC) and `pipeline.rs` (default into
`state.json`).
```

### Step 2.16: Tighten `## Logging` (last section in file)

- [ ] Replace lines 733–735 (entire `## Logging` section) with exactly this:

```markdown
## Logging

Bot processes log to stderr + `~/.right/logs/<agent>.log` (daily rotation
via `tracing-appender`). Aggregator logs to stdout +
`~/.right/logs/mcp-aggregator.log`. See: `docs/architecture/sessions.md`
for stream-logging detail.
```

### Step 2.17: Verify file size and content boundaries

- [ ] Run:

```bash
wc -c /Users/molt/dev/rightclaw/ARCHITECTURE.md
```

Expected: between **12,000 and 20,000 bytes** (target ~14,000–16,000). If significantly larger, check that all `MOVE` sections were actually deleted, not just commented.

- [ ] Run:

```bash
grep -c '^See: `docs/architecture/' /Users/molt/dev/rightclaw/ARCHITECTURE.md
```

Expected: **at least 11** (one per `MOVE`/`TIGHTEN` row that points at a satellite — Module Map, Agent Lifecycle, Voice transcription, OpenShell Sandbox, Login Flow, MCP Token Refresh, MCP Aggregator, Reflection, Stream Logging, Cron Schedule Kinds, Per-session mutex, Memory, Memory Resilience, Logging — count ≥11; some sections may share pointers).

- [ ] Run:

```bash
grep -E '^(##|###) ' /Users/molt/dev/rightclaw/ARCHITECTURE.md
```

Expected: every original `STAY`/`TIGHTEN` heading is still present. The `MOVE` headings (Module Map, Agent Lifecycle, Voice transcription, Login Flow, MCP Token Refresh, Stream Logging, Per-session mutex, Memory, Memory Resilience Layer) **must still appear as headings** — they were replaced with pointers, not deleted, so the heading remains followed by a 1–2 line pointer.

### Step 2.18: Verify no broken anchor references elsewhere

- [ ] Run:

```bash
cd /Users/molt/dev/rightclaw
rg 'ARCHITECTURE\.md#' --type md --type rust 2>/dev/null
```

Expected: results, if any, point only at headings that still exist in the trimmed `ARCHITECTURE.md`. To verify, for each result line `<file>:<line>:...ARCHITECTURE.md#<anchor>...`, slugify `<anchor>` and grep `ARCHITECTURE.md` for the matching heading. If any anchor's heading was deleted, leave a one-line pointer at that location in `ARCHITECTURE.md` so the link still resolves.

(Likely outcome: zero anchors. The earlier exploration showed only `2026-04-25-architecture-md-trim-design.md` referencing `ARCHITECTURE.md`, and not by anchor. No action expected.)

### Step 2.19: Commit

- [ ] Run:

```bash
cd /Users/molt/dev/rightclaw
git add ARCHITECTURE.md
git status
```

Expected: only `ARCHITECTURE.md` modified.

- [ ] Commit:

```bash
git commit -m "$(cat <<'EOF'
docs(arch): trim ARCHITECTURE.md to prescriptive content

Move descriptive sections to docs/architecture/*.md (created in previous
commit). Tighten OpenShell Sandbox Architecture, MCP Aggregator,
Prompting Architecture, Reflection Primitive, Cron Schedule Kinds,
PC_API_TOKEN authentication, and Logging to keep only the rules.

42 KB -> ~14-16 KB. Clears the SessionStart >=40KB warning.

See docs/superpowers/specs/2026-05-05-architecture-md-split-design.md.
EOF
)"
```

---

## Task 3: Clean up `CLAUDE.md` (Commit 3)

Drop the dependency-rationale block, add the "Architecture docs split" convention, and add a one-line Stack note under `### Constraints`.

**Files:**
- Modify: `/Users/molt/dev/rightclaw/CLAUDE.md`

### Step 3.1: Drop the dependency-rationale block (lines 21–110)

- [ ] In `CLAUDE.md`, delete **everything from line 21 (`## Technology Stack`) through line 110 inclusive (last line of `## Sources`).**

After deletion, the file goes directly from `### Constraints` (lines 13–19, which ends at line 19 with `OpenShell status` bullet) into what was previously `## Docs` at line 112. There will be only one blank line between `### Constraints` content and the next section.

### Step 3.2: Add Stack one-liner under `### Constraints`

- [ ] At the end of the `### Constraints` bullet list (after the `OpenShell status` bullet), append a new bullet:

```markdown
- **Stack**: `Cargo.toml` is the source of truth for dependencies. Project standards in `CLAUDE.rust.md`.
```

### Step 3.3: Insert "Architecture docs split" section

- [ ] Find the existing `## Architecture` section (originally line 128, now around line 23 after the deletions). Insert a new top-level section **immediately above it**, separated by a blank line:

```markdown
## Architecture docs split

`ARCHITECTURE.md` is **prescriptive only** — load-bearing rules, contracts,
gotchas, reference tables. It is `@`-imported and loads on every
conversation; every line costs tokens.

Descriptive content (data flows, feature mechanics, walkthroughs) lives in
`docs/architecture/*.md`. Reference these files by **plain path** in
`ARCHITECTURE.md` or here — never `@`-import them. That is the whole point
of the split.

When adding new content to `ARCHITECTURE.md`, ask: "is this a rule the
codebase enforces, or a description of how it works?" Rule →
`ARCHITECTURE.md`. Description → `docs/architecture/`.

**Cite-on-touch (mandatory):** when modifying a subsystem, re-read the
corresponding `docs/architecture/<x>.md` and update it if drifted. These
docs are not auto-loaded, so they will rot silently if not maintained.
Code is authoritative; the satellite doc is a courtesy to readers.
```

### Step 3.4: Verify file size and structure

- [ ] Run:

```bash
wc -c /Users/molt/dev/rightclaw/CLAUDE.md
```

Expected: between **3,500 and 6,000 bytes** (target ~5,000).

- [ ] Run:

```bash
grep -E '^##' /Users/molt/dev/rightclaw/CLAUDE.md
```

Expected sections (in order): `## Project`, `## Docs`, `## Conventions`, `## Architecture docs split`, `## Architecture`. The original `## Technology Stack`, `## Recommended Stack`, `## External Dependencies (Not Rust Crates)`, `## Integration Patterns`, `## Alternatives Considered`, `## Cargo.toml Dependencies`, `## Sources` must NOT appear.

- [ ] Run:

```bash
grep -A1 '^### Constraints' /Users/molt/dev/rightclaw/CLAUDE.md | head -20
```

Verify the `**Stack**` bullet is present in the Constraints list.

### Step 3.5: Verify `@`-imports are unchanged where they should be

- [ ] Run:

```bash
grep '^@' /Users/molt/dev/rightclaw/CLAUDE.md
```

Expected output (exactly):

```
@CLAUDE.rust.md
@ARCHITECTURE.md
```

(Two lines. No `@docs/architecture/...` lines — that would defeat the split.)

### Step 3.6: Commit

- [ ] Run:

```bash
cd /Users/molt/dev/rightclaw
git add CLAUDE.md
git status
```

Expected: only `CLAUDE.md` modified.

- [ ] Commit:

```bash
git commit -m "$(cat <<'EOF'
docs: trim CLAUDE.md and add Architecture docs split convention

- Drop the Technology Stack / Recommended Stack / Alternatives / Sources
  block (~90 lines). Cargo.toml and CLAUDE.rust.md are authoritative.
- Add cite-on-touch convention: descriptive subsystem docs live in
  docs/architecture/*.md, referenced by plain path (never @-imported).
  Re-read and update them when modifying a subsystem.

13 KB -> ~5 KB. Pairs with the ARCHITECTURE.md split in 0b38e8a2 / prior
commits.

See docs/superpowers/specs/2026-05-05-architecture-md-split-design.md.
EOF
)"
```

---

## Task 4: Final verification

Run the full success-criteria battery from the spec.

### Step 4.1: Size check

- [ ] Run:

```bash
wc -c /Users/molt/dev/rightclaw/ARCHITECTURE.md /Users/molt/dev/rightclaw/CLAUDE.md
```

Expected:
- `ARCHITECTURE.md`: ≤ 20,000 bytes (target ~14,000–16,000)
- `CLAUDE.md`: ≤ 6,000 bytes (target ~5,000)

If either is over budget, revisit the corresponding task and tighten further. Budget overruns are not blockers — the binding constraint is "≤ 40 KB on `ARCHITECTURE.md`".

### Step 4.2: No anchor references break

- [ ] Run:

```bash
cd /Users/molt/dev/rightclaw
rg 'ARCHITECTURE\.md#' --type md --type rust 2>/dev/null || echo "OK: no anchor references"
```

Expected: `OK: no anchor references`. If any results appear, verify each anchor still resolves to a heading in the trimmed `ARCHITECTURE.md`.

### Step 4.3: No `@`-imports of satellite docs

- [ ] Run:

```bash
cd /Users/molt/dev/rightclaw
rg '@docs/architecture' . 2>/dev/null || echo "OK: no @-imports of satellites"
```

Expected: `OK: no @-imports of satellites`. (A non-empty result would mean someone added a satellite to auto-loaded context, defeating the split.)

### Step 4.4: All satellites referenced from `ARCHITECTURE.md` exist

- [ ] Run:

```bash
cd /Users/molt/dev/rightclaw
grep -oE 'docs/architecture/[a-z]+\.md' ARCHITECTURE.md | sort -u
```

Expected: each path printed corresponds to a real file under `docs/architecture/`. Cross-check:

```bash
for p in $(grep -oE 'docs/architecture/[a-z]+\.md' ARCHITECTURE.md | sort -u); do
  test -f "$p" && echo "OK: $p" || echo "MISSING: $p"
done
```

Expected: all `OK:` lines, no `MISSING:`.

### Step 4.5: SessionStart smoke test

- [ ] In a new terminal, navigate to `/Users/molt/dev/rightclaw` and start `claude` (or your equivalent CLI). Verify the SessionStart hook **does not** fire the `Large ARCHITECTURE.md will impact performance` warning. (The trigger is ≥40 KB; we are well under.)

If the warning still fires: the trim was insufficient. Revisit Task 2 and remove more descriptive content.

### Step 4.6: Visual sanity check

- [ ] Open `ARCHITECTURE.md` and skim top to bottom. Every section either:
  - Contains a rule, contract, gotcha, or reference table (STAY/TIGHTEN), OR
  - Is a 1–2 line pointer to a satellite (MOVE).

There should be no remaining 50-line pseudocode blocks, 30-line schema dumps, or step-by-step bot-startup walkthroughs. If you find one, it slipped through — move it to the appropriate satellite and add a pointer.

### Step 4.7: Push (optional — only if user explicitly asks)

This plan creates three commits on `master`. **Do not push without the user's go-ahead.** When asked, run:

```bash
cd /Users/molt/dev/rightclaw
git push
```

---

## Self-review notes (for the engineer executing this plan)

- **All MOVE/TIGHTEN sections from the spec inventory are covered**: every row of the spec's section table maps to a numbered step in Task 1 (creation) and/or Task 2 (deletion/tighten). STAY rows have no associated steps — that's intentional, they don't change.
- **No placeholders**: every step contains either exact replacement text in a code block, a deletion command with line ranges, or a verification command with expected output.
- **Line numbers are anchors to the current `ARCHITECTURE.md`** as it exists at the start of execution. If any line drift between Task 1 and Task 2 is suspected (e.g., someone else commits to `ARCHITECTURE.md` in the interim), re-locate sections by their `###` heading rather than line number — the headings are the stable identifiers. (Task 1 does not modify `ARCHITECTURE.md`, so within a single execution session line numbers are stable.)
- **Three commits, not one**: this is deliberate. Reviewers can see the satellite content land first, then the trim, then the CLAUDE.md cleanup. `git log --reverse` reads cleanly.
