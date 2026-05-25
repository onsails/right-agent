# Legacy Learning Cleanup Design

## Context

Right Agent now has two learning histories in the codebase:

- The current skill-learning loop: prefilter, probe-writer, curator,
  `skill_learning_events`, `skill_lifecycle`, `curator_state`, and usage
  accounting.
- The deprecated Stage 2 learning path: reply nudge signals, learning episode
  seeds, selector/reviewer drains, report-only review rows, and dashboard views
  over historical episode/report data.

The deprecated path is already documented as disabled, but the repository still
contains no-op seed capture, dormant scheduler/reviewer code, legacy dashboard
read models, configuration compatibility fields, and database tables. This
cleanup removes the deprecated learning system completely, including historical
data, while preserving config-file upgrade compatibility.

This design covers learning-related legacy code only. It does not cover
unrelated compatibility code such as allowlist migration, OpenShell policy
migration, restore legacy path inference, or SQLite FTS5 scrubbing.

## Goals

- Remove deprecated Stage 2 learning runtime code.
- Drop historical deprecated learning tables/data by migration.
- Remove dashboard APIs and read models that depend on deprecated learning
  tables.
- Keep the dashboard learning views backed only by current learning sources:
  `skill_learning_events`, `skill_lifecycle`, `curator_state`, and usage data.
- Keep deprecated `agent.yaml` learning fields deserializable and warning-only.
- Update `ARCHITECTURE.md`, `PROMPT_SYSTEM.md`, and satellite architecture docs
  touched by this subsystem so they describe only the surviving learning model.
- Prove with tests that dropped tables are not queried at runtime.

## Non-Goals

- No archive tables for old learning data.
- No export flow for historical learning episodes or review reports.
- No removal of unrelated legacy migration helpers.
- No change to current probe-writer or curator behavior except where they still
  depend on deprecated review-gate state.
- No config parser hard failure for old ignored learning keys.

## Recommended Approach

Use a hard delete for deprecated learning data and code, with config
compatibility retained:

1. Add a migration that drops deprecated learning tables with
   `DROP TABLE IF EXISTS`.
2. Delete Stage 2 selector/reviewer runtime surfaces and the no-op plumbing that
   exists only to keep them dormant.
3. Remove dashboard routes, DTOs, and read-model fields that require dropped
   tables.
4. Rebuild the learning dashboard around current lifecycle, learning-event,
   curator-state, and usage sources only.
5. Keep deprecated learning config fields as `Option<_>` in `LearningConfig`
   and keep `warn_on_deprecated`.
6. Update docs and prompts to remove claims that historical learning reports are
   still dashboard data.

The migration is destructive by design. Existing `learning_episodes`,
`skill_nudge_signals`, `skill_nudge_state`, `skill_review_reports`, and related
data are removed from each per-agent `data.db`.

`execution_events` should be dropped if implementation confirms it has no
current non-legacy consumer. Current evidence points to legacy episode selection
and review evidence only, while the live learning system uses stream logs,
`skill_learning_events`, lifecycle rows, and usage rows. If a live consumer is
found, keep `execution_events` and document the surviving owner.

## Architecture

After cleanup, there is one learning pipeline:

```text
foreground turn
  -> optional learning_prefilter
  -> optional learning_probe_writer
  -> mcp__right__skill_learning_start / finish
  -> skill_learning_events + skill_lifecycle + usage

periodic curator
  -> curator_state + skill_lifecycle + usage
  -> mcp__right__skill_learning_start / finish
  -> skill_learning_events + skill_lifecycle + usage
```

Removed architecture:

- reply `learning_signal` and `skill_issue_signal` collection;
- `skill_nudge_signals` persistence;
- `skill_nudge_state` review gate and circuit state;
- `learning_episodes` seed capture;
- `DrainScheduler` and episode drain tasks;
- selector/reviewer Claude invocations;
- `skill_review_reports` writes and reads;
- learning episode list/detail dashboard APIs.

Deprecated config fields remain accepted:

- `fork_probe_enabled`
- `fork_probe_model`
- `probe_model`
- `background_review_enabled`
- `episode_selector_model`
- `episode_selector_max_budget_usd`
- `episode_settle_seconds`
- `circuit_failure_threshold`
- `circuit_cooldown_minutes`

They must not influence runtime behavior. They should continue to warn at load
time so existing `agent.yaml` files remain upgradeable without silently
pretending the fields still do anything.

## Dashboard

The dashboard must not rely on deprecated learning data.

Remove:

- learning episode list API;
- learning episode detail API;
- read-model modules that query `learning_episodes`;
- overview nodes/edges/counts sourced from `skill_nudge_signals`,
  `learning_episodes`, or `skill_review_reports`;
- tests and frontend/API assumptions that require those tables.

Keep or rebuild:

- current learning overview based on `skill_learning_events`;
- lifecycle counts and status based on `skill_lifecycle`;
- curator trigger/status data based on `curator_state`;
- learning spend based on usage sources;
- existing dashboard skill inventory and pinning flows.

Removed routes should disappear from the router so callers receive 404. A 500
caused by querying a dropped table is a regression.

## Database Migration

Add a new schema migration after the current latest migration. It should be
idempotent and safe for already-clean databases:

```sql
DROP TABLE IF EXISTS learning_episodes;
DROP TABLE IF EXISTS skill_nudge_signals;
DROP TABLE IF EXISTS skill_nudge_state;
DROP TABLE IF EXISTS skill_review_reports;
-- DROP TABLE IF EXISTS execution_events; only if no live consumer remains.
```

Indexes and triggers attached to these tables disappear with their tables. If
any standalone objects exist, the migration should drop them explicitly.

The migration should not rewrite user-managed `agent.yaml`.

## Error Handling

- Config compatibility warnings are non-fatal.
- Dropping absent tables is successful.
- Dashboard code must not catch missing-table errors as normal operation. Tests
  should fail if a removed read path still queries a dropped table.
- Current learning pipeline failures keep their existing behavior: log and skip
  for background learning work, without user-visible failure unless the existing
  path already reports one.

## Testing

Use TDD for behavior and schema changes:

1. Add migration tests proving the deprecated tables are absent after migration.
2. Add dashboard/read-model regression tests proving learning overview works
   without deprecated tables and episode APIs are absent.
3. Add config tests proving deprecated keys still deserialize and warn-only
   compatibility remains.
4. Delete old Stage 2 tests alongside the code they covered.

Targeted checks while implementing:

```bash
devenv shell -- cargo test -p right-db legacy_learning_cleanup
devenv shell -- cargo test -p right-dashboard learning
devenv shell -- cargo test -p right-bot dashboard
devenv shell -- cargo test -p right-agent-config learning_config_deprecated_fields_are_ignored
```

Final mandatory verification:

```bash
devenv shell -- cargo test --workspace
```

## Documentation

Update:

- `ARCHITECTURE.md`: remove the retained/deprecated Stage 2 architecture and
  describe the single surviving learning loop.
- `PROMPT_SYSTEM.md`: remove text about historical Stage 2 reports as active
  dashboard data.
- `docs/architecture/modules.md`: remove the learning episode read model and
  update dashboard module descriptions.
- `docs/architecture/sessions.md` and `docs/architecture/mcp.md`: remove
  report-only reviewer exceptions if their code is deleted.
- Any dashboard API docs or frontend assumptions that mention episode list or
  detail routes.

Docs should continue to mention deprecated config keys only as accepted legacy
input that warns and has no runtime effect.
