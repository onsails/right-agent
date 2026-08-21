# ARCHITECTURE.md split: prescriptive vs descriptive — Design

## Goal

Split `ARCHITECTURE.md` into two tiers:

1. **Prescriptive (stays auto-loaded)** — rules, contracts, gotchas, reference
   tables. Things that cannot be re-derived by reading code.
2. **Descriptive (moves to satellite docs)** — data flows, pseudocode,
   feature mechanics, walkthroughs. Things `git log` and the source are
   already authoritative for.

Drop `~25 KB` of dependency rationale from `CLAUDE.md` while we're in there.

**Targets:**

- `ARCHITECTURE.md`: 42 KB → ~14–16 KB.
- `CLAUDE.md`: 13 KB → ~5 KB.
- New satellite tree at `docs/architecture/*.md` (~24 KB total, not
  auto-loaded).

**Why now:** the SessionStart warning fires on `ARCHITECTURE.md` ≥ 40 KB.
Recent commits added the X-FORK-FROM / mutex / Immediate sections (`00f13939`)
and pushed it past the threshold. Every line in auto-loaded docs costs tokens
on every conversation turn.

## Relationship to prior spec

This supersedes
[`2026-04-25-architecture-md-trim-design.md`](./2026-04-25-architecture-md-trim-design.md).

That spec aimed for in-place tightening (~33–34 KB target) and **explicitly
rejected** satellite docs with the argument:

> Satellite reference docs rot faster than inline ones because they're not
> in the always-loaded context — and this project's "self-healing platform"
> ethos depends on the doc staying true.

The argument is real but the cost-benefit has shifted. Token cost per turn is
now the binding constraint, and we accept the rot risk by pairing the split
with a **cite-on-touch convention** in `CLAUDE.md`: when modifying a
subsystem, re-read its `docs/architecture/<x>.md` and update if drifted.

The 2026-04-25 spec stays in tree as historical record; do not delete.

## Scope

**In scope:**

- Edits to `/Users/molt/dev/rightclaw/ARCHITECTURE.md` (cuts + tighten +
  pointers).
- New tree at `/Users/molt/dev/rightclaw/docs/architecture/` with six files.
- Edits to `/Users/molt/dev/rightclaw/CLAUDE.md` (drop dependency block, add
  split convention).

**Out of scope:**

- `CLAUDE.rust.md`, `PROMPT_SYSTEM.md`, other docs.
- Code changes.
- Editorial rewrites of moved content (a future commit can tighten satellite
  files; the migration commit is a lossless move).
- Restructuring section ordering inside `ARCHITECTURE.md`.

## ARCHITECTURE.md inventory

Each section is classified **STAY** (prescriptive, no edit), **MOVE**
(descriptive, cut to satellite + leave plain-path pointer), or **TIGHTEN**
(extract prescriptive core, move the rest, leave pointer).

| Section | Lines (current) | Verdict | Destination |
|---|---|---|---|
| `## Workspace` | 3–11 | STAY | — |
| `## Module Map` | 13–42 | MOVE | `docs/architecture/modules.md` |
| `### Agent Lifecycle` | 46–152 | MOVE | `docs/architecture/lifecycle.md` |
| `### Voice transcription` | 153–172 | MOVE | `docs/architecture/lifecycle.md` |
| `### OpenShell Sandbox Architecture` | 173–209 | TIGHTEN | `docs/architecture/sandbox.md` |
| `### Login Flow (setup-token)` | 210–230 | MOVE | `docs/architecture/lifecycle.md` |
| `### MCP Token Refresh` | 231–243 | MOVE | `docs/architecture/mcp.md` |
| `### MCP Auth Types` | 244–254 | STAY | — |
| `### MCP Aggregator` | 255–274 | TIGHTEN | `docs/architecture/mcp.md` |
| `### Prompting Architecture` | 275–289 | TIGHTEN | (compress in place; pointer to PROMPT_SYSTEM.md) |
| `### Claude Invocation Contract` | 290–312 | STAY | — |
| `### Reflection Primitive` | 313–336 | TIGHTEN | `docs/architecture/sessions.md` |
| `### Stream Logging` | 337–352 | MOVE | `docs/architecture/sessions.md` |
| `### Cron Schedule Kinds` | 353–370 | TIGHTEN | `docs/architecture/sessions.md` |
| `### Per-session mutex on --resume` | 371–390 | MOVE | `docs/architecture/sessions.md` |
| `### Background continuation: X-FORK-FROM` | 391–406 | STAY | — |
| `### Configuration Hierarchy` | 407–420 | STAY | — |
| `### Memory` | 421–463 | MOVE | `docs/architecture/memory.md` |
| `### Memory Resilience Layer` | 464–484 | MOVE | `docs/architecture/memory.md` |
| `### Memory Schema (SQLite)` | 485–491 | STAY | — |
| `## External Integrations` | 492–505 | STAY | — |
| `## Runtime isolation — mandatory` | 506–522 | STAY | — |
| `### PC_API_TOKEN authentication` | 523–543 | TIGHTEN | (compress in place; rule stays) |
| `## SQLite Rules` | 544–559 | STAY | — |
| `## Upgrade & Migration Model` | 560–650 | STAY | — |
| `## Integration Tests Using Live Sandboxes` | 651–675 | STAY | — |
| `## Security Model` | 676–688 | STAY | — |
| `## Brand-conformant CLI output` | 689–701 | STAY | — |
| `## OpenShell Integration Conventions` | 702–709 | STAY | — |
| `## OpenShell Policy Gotchas` | 710–721 | STAY | — |
| `## Directory Layout (Runtime)` | 722–732 | STAY | — |
| `## Logging` | 733–735 | TIGHTEN | (compress to one line) |

### Tighten rules per section

- **OpenShell Sandbox Architecture:** keep one paragraph stating "sandboxes
  are persistent — never deleted automatically" and "policy hot-reload via
  `openshell policy set --wait` for network only; filesystem changes require
  recreation". Move all diagrams and step-by-step prose to `sandbox.md`.
- **MCP Aggregator:** keep the tool-routing prefix table (`{server}__`,
  `rightmeta__`, no prefix) and the internal-API endpoint list. Move the
  prose explanation of tool dispatch to `mcp.md`.
- **Prompting Architecture:** compress to three lines pointing at
  `PROMPT_SYSTEM.md` and stating that `--system-prompt-file` is the sole
  prompt mechanism (no `--agent` flag).
- **Reflection Primitive:** keep the rule "on CC failure the worker and cron
  call `reflect_on_failure`". Move worker/cron limit specifics
  (`ReflectionLimits::WORKER`, `CRON`) and label-routing detail to
  `sessions.md`.
- **Cron Schedule Kinds:** keep the `Immediate` sentinel + 6h-default
  `lock_ttl` invariant (these are contracts code relies on). Move enum
  variant descriptions to `sessions.md`.
- **PC_API_TOKEN authentication:** keep the rule "always resolve through
  `PcClient::from_home(home)`; never import `PC_PORT` directly". Drop the
  mechanism prose; the rule alone is sufficient.
- **Logging:** compress to "Bot processes log to stderr + per-agent daily
  rotation. Aggregator logs to stdout + rotation. See
  `docs/architecture/sessions.md` for stream-logging detail."

## Satellite docs

Six files under `docs/architecture/`. Each starts with this header:

```markdown
> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `CLAUDE.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.
```

| File | Source sections from old `ARCHITECTURE.md` | Target size |
|---|---|---|
| `modules.md` | Module Map (right-agent / right / right-bot) | ~3 KB |
| `lifecycle.md` | Agent Lifecycle + Voice transcription + Login Flow | ~7 KB |
| `sandbox.md` | OpenShell Sandbox Architecture (descriptive parts) | ~2 KB |
| `mcp.md` | MCP Token Refresh + MCP Aggregator (descriptive parts) | ~2 KB |
| `memory.md` | Memory + Memory Resilience Layer | ~5 KB |
| `sessions.md` | Stream Logging + Per-session mutex + Reflection detail + Cron Schedule Kinds detail | ~5 KB |

Rules each satellite file follows:

1. **Plain markdown only.** No `@`-imports anywhere — the whole point of the
   split is that these files are NOT auto-loaded.
2. **Reference by plain path.** `ARCHITECTURE.md` and any other doc reference
   satellites via `docs/architecture/<name>.md` (no `@` prefix, no
   `[]()` link required — though links are fine).
3. **Lossless move in the migration commit.** Cut, paste, no editorial
   rewrite. Future commits can tighten.

## CLAUDE.md changes

### Drop the dependency-rationale block

Delete lines 21–110 of current `CLAUDE.md`. That covers `## Technology
Stack`, the empty `## Async vs Sync Decision` header, all `## Recommended
Stack` per-tool tables, `## External Dependencies (Not Rust Crates)`, `##
Integration Patterns` with its empty stubs and stranded `# comment` lines,
`## Alternatives Considered`, `## Cargo.toml Dependencies`, and `## Sources`.

Replace with a single line, placed under `### Constraints`:

> **Stack:** `Cargo.toml` is the source of truth for dependencies. Project
> standards in `CLAUDE.rust.md`.

### Add "Architecture docs split" convention

Insert this new top-level section **immediately above** the existing
`## Architecture` section (which contains `@ARCHITECTURE.md`):

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
docs are not auto-loaded, so they will rot silently if not maintained. Code
is authoritative; the satellite doc is a courtesy to readers.
```

The existing `## Project` paragraph (lines 5–11) and `### Constraints`
section header stay verbatim.

## Migration approach

One atomic commit per logical step, not one giant commit.

1. **Commit 1 — Create satellite files:** add the six `docs/architecture/*.md`
   files. Each contains the lossless-moved sections (cut from `ARCHITECTURE.md`)
   plus the status header. `ARCHITECTURE.md` is **not yet edited** in this
   commit, so the moved content lives in two places briefly. Rationale:
   reviewers can diff the satellites against the original sections without
   `git log -p` gymnastics.
2. **Commit 2 — Trim `ARCHITECTURE.md`:** delete moved sections, apply
   tighten edits, insert `See: docs/architecture/<x>.md` pointers where
   sections used to be. After this commit, no content is duplicated.
3. **Commit 3 — `CLAUDE.md` cleanup:** drop dependency block, add split
   convention.

If reviewer feedback requires changes inside satellites, those happen in
follow-up commits. Migration commits do not editorialize.

## Success criteria

- `ARCHITECTURE.md` ≤ 20 KB (target ~14–16 KB).
- `CLAUDE.md` ≤ 6 KB (target ~5 KB).
- No `ARCHITECTURE.md#anchor` references elsewhere in the repo break. Verify
  via `rg 'ARCHITECTURE\.md#' --type md --type rust` before each commit.
- No `@`-imports of `docs/architecture/*.md` anywhere. Verify via
  `rg '@docs/architecture' --type md`.
- Every section moved out of `ARCHITECTURE.md` is reachable via a
  plain-path pointer left behind (no orphaned moves).
- New top section in `CLAUDE.md` called "Architecture docs split" is in
  place above `## Architecture`.
- The dropped dependency-rationale block is gone, replaced by the one-line
  Stack note under `### Constraints`.
- Smoke test: open a fresh terminal, run `claude` in the repo root, verify
  the SessionStart warning about `ARCHITECTURE.md` ≥ 40 KB no longer fires.

## Risks and mitigations

- **Satellite docs rot silently.** Mitigation: cite-on-touch rule in
  `CLAUDE.md`. Code is authoritative; satellites are a courtesy. Do not add
  satellite content to `ARCHITECTURE.md` "just in case it gets stale".
- **Reviewer can't tell whether moved content changed.** Mitigation:
  two-commit migration (create satellites first, then trim source). Diffs
  are mechanical.
- **Future contributors add new descriptive content to `ARCHITECTURE.md`
  out of habit.** Mitigation: the split convention in `CLAUDE.md` plus the
  tighten/move precedent set by this change. No automation; rely on
  convention discipline.
- **Anchors in external links break (specs reference `ARCHITECTURE.md#x`).**
  Mitigation: pre-commit `rg` sweep listed under success criteria. If a
  matching anchor moves, leave a one-line pointer at the old location.

## Non-goals

- Editorial tightening inside satellite files in the migration commit.
  Future commits handle that.
- Changing how `@ARCHITECTURE.md` is imported from `CLAUDE.md` (still a
  plain `@`-import).
- Generating cross-reference tables, indexes, or a TOC.
- Updating `PROMPT_SYSTEM.md`, `CLAUDE.rust.md`, or other docs.

## Cross-references

- `CLAUDE.md` → "Self-healing platform", "Upgrade-friendly design" —
  conventions whose spirit the cite-on-touch rule preserves.
- `2026-04-25-architecture-md-trim-design.md` — superseded; rationale
  difference recorded above.
