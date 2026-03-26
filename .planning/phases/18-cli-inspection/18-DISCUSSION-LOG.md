# Phase 18: CLI Inspection - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-26
**Phase:** 18-cli-inspection
**Areas discussed:** Subcommand shape, Output format, Pagination, Delete semantics

---

## Subcommand Shape

| Option | Description | Selected |
|--------|-------------|----------|
| Nested subcommand | `rightclaw memory list <agent>` — mirrors existing `config` pattern, groups all 4 ops under one --help | ✓ |
| Flat top-level | `rightclaw memory-list <agent>` etc. — avoids nested enum but pollutes top-level --help | |

**User's choice:** Nested subcommand
**Notes:** Consistent with `Commands::Config` pattern already in codebase.

---

## Output Format

| Option | Description | Selected |
|--------|-------------|----------|
| Plain columnar text | Fixed-width columns, no deps, matches existing `rightclaw list` style | |
| Plain text + --json flag | Default is columnar; `--json` emits newline-delimited JSON for scripting | ✓ |
| Always JSON | No human-readable table | |

**User's choice:** Plain text + `--json` flag
**Notes:** `--json` flag on `list`, `search`, and `stats`. JSON keys match `MemoryEntry` field names.

---

## Pagination

| Option | Description | Selected |
|--------|-------------|----------|
| --limit N, default 50 | Single limit arg with footer showing total count | ✓ |
| --limit + --page | Friendlier page alias for offset math | |
| No pagination | Return everything | |

**User's choice:** `--limit`/`--offset`, but **default limit = 10** (not 50 as presented)
**Notes:** User explicitly set default to 10. Footer: `10 of 127 entries shown  (--offset 10 for next page)`. Footer suppressed when `--json` active.

---

## Delete Semantics

### Hard-delete depth

| Option | Description | Selected |
|--------|-------------|----------|
| memories row only | DELETE memories row; memory_events preserved (audit trail intact) | ✓ |
| Full cascade | DELETE memories + all memory_events rows | |

**User's choice:** `memories` row only — audit trail preserved in `memory_events`.

### Confirmation mechanics

| Option | Description | Selected |
|--------|-------------|----------|
| Always prompt | Show entry content, ask `Hard-delete? [y/N]`. Default No. | ✓ |
| Prompt + --force/-f | Default prompts; `--force` skips for scripting | |

**User's choice:** Always prompt (no `--force` flag in v2.3).

---

## Claude's Discretion

- Column widths and content truncation in list output
- `stats` size display format (bytes vs auto-scaled KB/MB)
- Exact SQL for stats query

## Deferred Ideas

- `--force`/`-f` flag on delete — v2.4 if scripting need emerges
- `rightclaw memory export` — MEM-F04 in REQUIREMENTS.md
