# Cron ↔ Skill Linking — Design

**Date:** 2026-06-15
**Status:** Draft (brainstorm output, pre-plan)
**Owner:** andrey

## Problem

A user creates a recurring cron (e.g. "find sources for news hooks and write
articles"). Over its runs the learning pipeline distills a `rightx-*` skill that
captures the procedure — but the cron's stored prompt is frozen and nothing tells
the cron to *rely* on that skill. Today the only coupling is implicit: Claude Code
auto-discovers all `rightx-*` skills in the sandbox by their `SKILL.md`
`description`, so the skill is *available* but its use is a gamble on
description-matching, not a guarantee. There is also no record of *which* skills
belong to *which* cron.

We want **explicit, deterministic linking** of one or more skills to a cron, so:

1. A cron deterministically pulls its linked skills on every run (no
   description-matching roulette).
2. The agent can grow a cron from a fat hand-written prompt toward a thin "what"
   prompt that defers to linked skills — an **auditable, agent-driven evolution**,
   never a silent platform rewrite.

Peer platform **Hermes** links crons to skills (multiple at once); this is the
RightClaw equivalent, deliberately shaped to the project's auditable-by-design
ethos.

### What the prior research established (and one correction)

- **No code path mutates `cron_specs.prompt` as a consequence of learning.** The
  prompt column is writable only via `cron_spec.rs:463`/`:575`, reachable from the
  agent/operator `cron_update`/`cron_create` MCP tools. The learning pipeline has
  zero cron-prompt coupling. (Confirmed.)
- **The learned skill already auto-loads** on the next cron run via the sandbox
  skill index — so the functional benefit largely exists; the residual is
  reliability + token bloat + missing provenance.
- **Correction to the workflow summary:** the async probe-writer registers its
  *own* invocation with `kind: ProbeWriter` (`learning_probe_writer.rs:185`), so a
  cron-triggered probe-written skill gets `created_by = ProbeWriter`,
  **indistinguishable from a foreground-authored skill**. Only *inline* cron
  authoring yields `created_by = 'cron'`. Therefore `created_by` cannot express
  cron provenance, and a dedicated link table is the only reliable record.

## Goals

- Persisted **many-to-many** link: a cron has many skills; a skill may be linked
  to many crons (agent cross-linking).
- **Auto-link** every skill a recurring cron's own runs create or patch.
- **Agent-managed** linking via MCP: link a set at `cron_create` time, and
  `cron_link_skill` / `cron_unlink_skill` for existing crons.
- **Deterministic runtime pull**: each run names its linked (live) skills as
  authoritative.
- **Agent-driven prompt evolution**: the `right-cron` skill instructs the agent to
  slim a cron's prompt toward linked skills when editing it.
- Backward-compatible upgrade for deployed agents (migration only; empty table =
  prior behavior).

## Non-goals (v1, deliberate)

- **No silent platform rewrite of `cron_specs.prompt`.** Prompt mutation stays
  exclusively on the agent/operator `cron_update` path. (Auto-rewrite is the one
  move the project ethos forbids; the 2026-06-14 cron-then-continuation spec
  already documents the churn it causes.)
- **No proactive bot-message / platform trigger** nudging "this cron grew a skill,
  slim its prompt." Evolution is *reactive*: it fires when the agent is already
  editing or reasoning about the cron (skill guidance + `cron_list` introspection).
  Rationale: the functional benefit is delivered immediately by auto-link + runtime
  directive; prompt slimming is token-only and non-urgent, so lazy evolution is
  correct and churn-free. (The proactive nudge is deferred to v2 — tracked in
  **onsails/right-agent#128**.)
- **No cron self-rewrite** mid-run.
- **No dashboard linking surface** (the agent/MCP is the control plane for this
  feature per the user's scope).
- **No "exclude" state** for unlinked-then-re-learned skills (see Edge cases).

## Division of responsibility (the heart of the design)

Two mechanisms, distinct roles — do not conflate:

| Mechanism | When | Role |
|---|---|---|
| **Platform runtime directive** (`compose_run_prompt`) | every run, from the link table | *Immediate determinism.* A freshly auto-linked skill is named as authoritative on the very next run — before any prompt evolution. |
| **Agent prompt evolution** (`right-cron` skill → `cron_update`) | when the agent edits/reasons about the cron | *Gradual, auditable token cleanup.* Slims the fat stored prompt to "what" + skill references. |

The directive gives the user what they asked for ("the cron knows to pull this
skill") without waiting on evolution; evolution is the optional cleanup that the
directive makes safe.

## Data model

New migration in `right_db::migrations::MIGRATIONS` — highest existing is `v46`,
so this is `v47_cron_skill_links.sql` — idempotent:

```sql
CREATE TABLE IF NOT EXISTS cron_skill_links (
  job_name   TEXT NOT NULL,                         -- cron_specs.job_name (per-agent natural key)
  skill_name TEXT NOT NULL,                         -- skill_lifecycle.skill_name (rightx-*)
  origin     TEXT NOT NULL CHECK (origin IN ('auto','agent')),
  created_at TEXT NOT NULL,
  PRIMARY KEY (job_name, skill_name)
);
CREATE INDEX IF NOT EXISTS idx_cron_skill_links_skill ON cron_skill_links(skill_name);
```

- PK `(job_name, skill_name)` ⇒ idempotent upsert, native many-to-many.
- `idx ... (skill_name)` serves the reverse lookup the curator needs on
  absorb/retire.
- Per-agent `data.db` — no cross-agent reach (job_name is unique within an agent).

**Helpers — new module `right_agent::cron_skill_link`** (next to `cron_spec.rs`;
SQL over the `right_db` connection, same pattern):

- `upsert(conn_or_tx, job, skill, origin)` → `INSERT ... ON CONFLICT DO NOTHING`
  (auto never overwrites an existing agent link).
- `unlink(conn_or_tx, job, skills: &[String])`.
- `list_for_job(conn, job) -> Vec<String>` and a `list_live_for_job` variant that
  joins `skill_lifecycle` and filters `state != 'archived'`.
- `jobs_for_skill(conn, skill) -> Vec<String>` (reverse).
- `redirect_skill(tx, old, new)`, `drop_skill(tx, skill)` (curator maintenance).
- `delete_for_job(tx, job)` (cron deletion).

Placement rationale: `right` already depends on `right-agent`
(`right_backend.rs:421`), and `bot` depends on `right-agent` (`cron_spec`), so all
writers/readers reach the module with no new dependency edges or cycles.

## Write paths

### Auto-link (origin `'auto'`) — two localized seams

`ProbeAnchor` (`crates/bot/src/telegram/worker.rs:210`) gains
`origin_cron_job: Option<String>`. Cron sets `Some(job_name)` at the anchor build
(`cron.rs:1256`); the foreground worker sets `None` (`worker.rs:1989`).

A skill authored from a cron run is written by exactly one of two paths (the
pipeline early-returns on inline authoring, `learning_pipeline.rs:70-76`, so the
two never both fire for one turn):

1. **Inline authoring** (cron agent writes a skill mid-turn, under
   `cron_invocation_id`, `created_by='cron'`). Linked in `cron.rs`: inside the
   existing recurring-success learning block (`cron.rs:1247-1309`), after the run,
   query successful finishes for `cron_invocation_id` and upsert a link per skill.
2. **Async probe-writer** (separate fork, own `ProbeWriter` invocation,
   `created_by='ProbeWriter'`). Linked in the probe-writer spawn tail
   (`learning_probe_writer.rs:328-343`), which already resolves the authored skill
   via `finish_event_for_invocation`. Thread `anchor.origin_cron_job` into the tail
   (capture it before `tokio::spawn`, like `invocation_id`); when `Some(job)` and
   the finish status is `created`/`updated`, upsert the link.

New `right_agent::learned_skills` helper
`successful_finishes_for_invocation(conn, invocation_id) -> Vec<(skill_name, status)>`
(plural sibling of `successful_finish_exists`) backs both seams.

This keeps the entire auto-link change inside `bot` + `right-agent`: no
`internal_client` DTO, progress-registry, or `right_backend` finish-handler
changes, and no cross-crate transaction composition with `right-lifecycle`.

### Agent MCP

- **`cron_create` gains `skill_names: Option<Vec<String>>`** — after
  `create_spec_v2`, upsert each as `origin='agent'`. Matches the skill's existing
  "capture the skill *before* creating the cron" guidance, so the skills already
  exist at create time.
- **`cron_link_skill { job_name, skill_names: [..] }`** and
  **`cron_unlink_skill { job_name, skill_names: [..] }`** — batch ("several at
  once"), `origin='agent'` on link. Handlers in `right_backend.rs` +
  `memory_server.rs` (the two cron-tool surfaces). Validate `job_name` exists in
  `cron_specs` and each `skill_name` exists in `skill_lifecycle`
  (`state != 'archived'`); reject unknown. Scope is server-resolved: a per-agent
  MCP only ever sees its own crons/skills (same scoping as `cron_update`).
- Not folded into `cron_update`: its partial-RMW "unspecified = unchanged"
  semantics do not express add/remove of a set.

### Transaction rule

- Inline seam and `cron_create` with N skills perform 2+ writes → wrap the upserts
  (and the adjacent spec write, for `cron_create`) in one immediate transaction.
- Async seam: the `skill_spend` insert + the single link upsert → one transaction.
- Cron deletion: fold `delete_for_job` into the shared `cron_spec::delete_spec`
  (`cron_spec.rs:693`) transaction — both the `cron_delete` tool and the one-shot
  auto-delete (`cron.rs:2065-2073`) route through it, so one change covers both.
- Single-skill, single-statement upserts on their own need no transaction.

## Runtime surfacing

In `compose_run_prompt` (`cron.rs:190-216`), **only when the job has ≥1 live
link**, append a terse block to the per-run user message (not the system prompt —
this is per-invocation dynamic content, consistent with stdin-side placement for
session-scoped data):

```
## Linked skills
Linked skills for this job — use them via the Skill tool as appropriate: rightx-source-finder, rightx-article-writer
```

- **Names only.** CC already holds every dir skill's `description` in context; the
  link adds a prioritization signal ("these are authoritative for this job"), not a
  content dump — preserves progressive disclosure and prompt-tier brevity.
- Read via `list_live_for_job` (`state != 'archived'`) so a retired/absorbed skill
  is never named.
- "as appropriate" phrasing: available, not mandatory — a run uses the relevant
  subset by judgment; we cannot force-load, only direct.

## Introspection — `cron_list`

`cron_list` output gains `linked_skills: [..]` per job (from `list_for_job`). This
is **required**, not cosmetic: it is what makes the agent's evolution self-check
possible — the agent edits/reasons about a cron, sees its linked skills, and
decides whether to slim the prompt. Update the `right-agent` cron list function and
both handlers.

## `right-cron` skill (bump `3.6.0` → `3.7.0`)

`crates/right-codegen/skills/right-cron/SKILL.md` is a codegen template
(`Regenerated(BotRestart)`), deployed on restart — upgrade-friendly. Edits:

1. **New section "Linking skills to a cron"**: auto-link (skills learned by a
   cron's own runs attach automatically); `skill_names` on `cron_create`;
   `cron_link_skill` / `cron_unlink_skill` for existing crons; and that linked
   skills are pulled directively at fire time. Reinforce the existing "What, not
   how" section (lines 101-108), which already preaches goal-prompt + skill-procedure.
2. **Prompt evolution at edit time** — woven into "Editing a Cron Job"
   (lines 132-148), roughly:
   > Before `cron_update`-ing a prompt, check the job's `linked_skills`
   > (`cron_list`). If the prompt still spells out "how" that a linked skill now
   > covers, slim the prompt to the "what" and rely on the link. The goal is
   > migrating a fat cron prompt toward a thin "what" + skill references. Tell the
   > user what you simplified — never a silent rewrite.

   This is the only sanctioned path that mutates `cron_specs.prompt`: agent
   judgment, via `cron_update`, visible to the user. The platform never touches the
   prompt.
3. **Parameters table**: add a `skill_names` row for `cron_create`.

Side benefit to note (do not overstate): when the agent slims the *stored* prompt
to reference skills, the learning anchor (`user_msg_text = spec.prompt`,
`cron.rs:1257`) then contains the skill names, biasing the prefilter toward
`PatchExisting` over `CreateNew` — improving dedup. (The platform runtime directive
lives in `compose_run_prompt`'s output, not in `spec.prompt`, so it does *not* enter
the anchor.)

## Lifecycle integrity

- **Curator** (`learning_curator.rs:370`, after `archived_skill_names` is
  computed): per archived skill, `redirect_skill(old → absorbed_into)` when
  `absorbed_into` is set, else `drop_skill`. The runtime `state != 'archived'`
  filter is the correctness backstop, so strict atomicity here is not required (it
  also covers multi-hop absorb chains, which converge as each pass redirects).
  Confirm `absorbed_into` population during execution.
- **Cron deletion**: `delete_for_job` folded into the shared `cron_spec::delete_spec`
  transaction (see Transaction rule) — covers tool + one-shot in one place.

## Edge cases

- **Unlink vs re-auto-link**: `cron_unlink_skill` is point-in-time; if the cron
  later create/patches that skill again, auto-link re-adds it. No permanent
  exclusion in v1 (would need an `excluded` state — deferred as YAGNI). Document so
  it is not surprising.
- **`job_name` reuse**: deletion cascades links, so a recreated same-named cron
  starts clean.
- **One-shot crons**: never auto-link (learning is recurring-only); agent linking
  is allowed but pointless (single run) — not blocked, not special-cased; links are
  cleaned by the auto-delete cascade.
- **Linked skill missing on disk at run time** (synced row, absent file): the
  directive names it, CC simply doesn't find it; the cron does **not** fail (soft
  degradation, self-heals on next sync).

## Upgrade & codegen category

- The link table is **runtime `data.db` state**, not a codegen output — it is a
  migration, *not* a `codegen_registry()` entry. Both bot and mcp-server run
  bootstrap, so the table appears on first startup after upgrade.
- Backward-compatible: empty table ⇒ no runtime block, no behavior change.
  Deployed agents adopt via `right restart` — no `right agent init`, no sandbox
  recreation.

## Docs & conventions to update

- `PROMPT_SYSTEM.md`: document the `## Linked skills` runtime block and the cron
  prompt-evolution guidance.
- `ARCHITECTURE.md`: minimal prescriptive addition only (MCP link-tool scope is
  server-resolved; link writes share the adjacent write's transaction). Walkthrough
  goes in the `docs/architecture/learning.md` satellite, referenced by plain path.
- `with_instructions()` in **both** `memory_server.rs` and `aggregator.rs`: add
  `cron_link_skill` / `cron_unlink_skill` and the `skill_names` param.
- MCP tool-name references (full `mcp__right__` prefix) in `skills/`,
  `templates/right/`, codegen, `PROMPT_SYSTEM.md`.

## Verification cadence

- **Baseline** at worktree start: targeted build of touched crates.
- **Targeted (TDD red/green)** per slice:
  - `right-db`: migration idempotency + `registry`/migration tests.
  - `right-agent`: `cron_skill_link` helpers (idempotent upsert; `list_live_for_job`
    excludes archived; redirect; drop; `delete_for_job`); `successful_finishes_for_invocation`.
  - `bot`: `compose_run_prompt` injects the block only with ≥1 live link; auto-link
    seams (inline + async) write rows; one-shot/`cron_delete` cascade.
  - MCP: `cron_create` `skill_names`, `cron_link_skill`/`unlink` validation + scope;
    `cron_list` returns `linked_skills`.
  - curator: absorb redirects, archive filtered at runtime.
  - Live create-then-reuse (a recurring cron learns a skill → row linked → named on
    next run): `ci_claude_`-prefixed `#[ignore = "ci-claude: ..."]` integration test
    per `AGENTS.rust.md` (also closes the `cron.rs:3828` `todo!()` runbook stub).
- **Final (mandatory, in-worktree)**: `devenv shell -- cargo nextest run --workspace`
  plus `devenv shell -- cargo test --doc --workspace`.

## Resolved decisions

- Evolution is **reactive** in v1 (no proactive nudge). The proactive
  "this cron grew a skill, consider slimming" nudge — pull-only, surfaced to the
  agent, never a silent rewrite — is deferred to v2, tracked in
  **onsails/right-agent#128**.
