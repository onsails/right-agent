# Dashboard v2 Miniapp Design

## Goal

Upgrade the Telegram Mini App dashboard from a cron/learning monitor into a
read-only operator surface for one Right Agent.

The dashboard should answer these questions without CLI access:

- What is running or failing?
- What did cron/background/foreground runs do recently?
- What is the agent spending?
- What has the agent learned, and which skills are available?
- What are the current identity files?
- Is the agent runtime healthy?

## Current System Context

`right-dashboard` already owns Telegram Mini App DTOs, initData auth helpers,
SQLite read models, frontend source, and checked-in static assets.
`right-bot::telegram::dashboard` mounts the routes and owns bot token custody,
allowlist authorization, agent path checks, runtime state, and static serving.

The current dashboard exposes read-only cron/run APIs and learned-skill report
metrics. The current frontend is a single Vue `App.vue` that is already large,
so Dashboard v2 should introduce focused frontend view components instead of
continuing to grow one file.

## Selected Approach

Use explicit dashboard domains:

```text
Overview | Activity | Knowledge | Usage | Identity | Health
```

Keep the dashboard read-only. The Mini App may refresh live read snapshots, but
it must not mutate cron specs, skills, identity files, sandbox processes,
agent config, MCP config, credentials, or database rows.

This approach is better than a compact `System` tab because skills and learning
are knowledge surfaces, not runtime diagnostics. It is better than an
overview-only dashboard because usage, skills, identity files, and run history
need dense inspection views.

## Scope

In scope:

- Top-level Dashboard v2 navigation.
- `Activity` view for cron/background run history and active foreground state.
- `Knowledge` view with `Learning` subviews for real episodes and reports.
- `Knowledge / Skills` grouped by core, learned, and other installed skills.
- `Usage` view replacing the pretty `/usage` text report.
- `/usage` removed from Telegram command autocomplete; manual `/usage` sends an
  open-dashboard prompt instead of the old summary.
- `Identity` view for `IDENTITY.md`, `SOUL.md`, and `USER.md`.
- `Health` view for live doctor results and sandbox stats.
- Backend capabilities so the frontend only renders available read-only views.
- Bounded payloads for logs, evidence, process lists, skill previews, and
  identity files.

Out of scope:

- Any write operation.
- Cron create/edit/delete/trigger/stop/retry.
- Skill install/update/remove/edit.
- Identity editing.
- Sandbox process control.
- MCP server management.
- Memory dashboard implementation. `Knowledge` is designed to accept Memory
  later, but this spec does not add it.
- Multi-agent dashboard.
- Raw unrestricted file browser or log browser.
- Live WebSocket/SSE streaming.

## Product Shape

### Overview

`Overview` is a compact landing screen, not a duplicate of every table.

It shows:

- active run count;
- recent failures;
- today's cost;
- learning candidate count;
- doctor status as not-loaded or last client-fetched snapshot, without causing
  an implicit doctor run;
- sandbox availability summary.

### Activity

`Activity` owns cron/background run visibility plus active foreground state.

It shows:

- cron specs and schedules;
- active cron/background runs;
- active foreground sessions;
- recent cron/background run history;
- delivery state;
- completed cost;
- run detail with capped log/event excerpt.

Existing cron dashboard functionality moves under this domain.

### Knowledge

`Knowledge` owns learning and skills.

`Learning` has two subviews:

- `Episodes`: real `learning_episodes` rows with status, trigger, kind,
  bounds, selected refs, context-incomplete flag, and linked report state.
- `Reports`: existing review reports with candidate status, confidence,
  evidence snippets, reviewer detail, and a link back to the episode when one
  exists.

`Skills` shows runtime-available skills grouped as:

- `core`: names in `right-codegen::BUILTIN_SKILL_NAMES`;
- `learned`: `rightx-*` skills or registry entries with source `learned`;
- `other`: hub, manual, or otherwise installed skills.

Skill detail shows bounded read-only `SKILL.md` preview and metadata. It does
not expose arbitrary package files.

### Usage

`Usage` replaces `/usage` output with structured visuals.

It shows windows for today, last 7 days, last 30 days, and all time, split by
source:

- interactive;
- cron;
- reflection.

Each window includes retail cost, subscription/API split, turns, invocation
count, token/cache/web counters, and per-model totals.

### Identity

`Identity` is a top-level tab.

It shows read-only:

- `IDENTITY.md`;
- `SOUL.md`;
- `USER.md`.

For sandboxed agents, the dashboard reads live sandbox files first. If sandbox
read fails, it falls back to the host mirror and reports the source and warning.
Missing files are represented per file, not as a whole-tab failure.

### Health

`Health` contains live diagnostics:

- `Doctor`: on-demand `right_agent::doctor::run_doctor(home)` results grouped
  by pass/warn/fail. The existing `/doctor` slash command remains.
- `Sandbox`: bounded in-sandbox probes for disk free, CPU/RAM snapshot, and
  process list inside the sandbox.

Process-compose platform process state is not part of this spec. The sandbox
process list means processes visible inside the agent sandbox.

## Backend Architecture

Keep the current ownership boundary:

- `right-dashboard` owns DTOs, auth validation helpers, SQLite read models,
  frontend source, and static assets.
- `right-bot` owns route mounting, Telegram auth enforcement, allowlist
  authorization, agent path checks, bot token custody, runtime sandbox access,
  and live doctor execution.

SQLite-only read models belong in `right-dashboard`.

Live runtime reads belong in `right-bot::telegram::dashboard` or helper modules
called from that route layer because the bot already has `RIGHT_HOME`,
resolved sandbox context, and per-agent runtime state.

`right-dashboard` may define DTOs for live runtime payloads, but it must not
become a process owner, sandbox owner, process-compose client, or Teloxide
dispatcher owner.

## API Routes

All routes are under:

```text
/dashboard/<agent>/api/v1
```

Routes:

```text
GET /bootstrap
GET /overview

GET /activity/overview
GET /activity/runs/{run_id}

GET /knowledge/learning/overview
GET /knowledge/learning/episodes
GET /knowledge/learning/episodes/{episode_id}
GET /knowledge/learning/reports/{report_id}
GET /knowledge/skills
GET /knowledge/skills/{skill_name}

GET /usage

GET /identity
GET /identity/{file_name}

GET /health/doctor
GET /health/sandbox
```

`<agent>` must match the running bot's agent name. Cross-agent routing remains
forbidden.

`bootstrap.features` should expose read-only capability booleans:

```text
activity
knowledge_learning
knowledge_skills
usage
identity
doctor
sandbox_stats
commands_enabled = false
learning_commands = false
```

Future write capabilities must be separate booleans and require a separate
design before routes are exposed.

## Data Sources

### Activity

Use:

- `cron_specs`;
- `async_runs`;
- `usage_events`;
- foreground stop-token map from the bot.

Run detail keeps the current capped log excerpt behavior. Missing logs return
an unavailable log section, not a failed run-detail response.

Foreground activity is active-only in this spec. It should not be presented as
the same persisted run history as `async_runs` unless implementation adds a
separate persisted foreground-run model in a later design.

### Learning

Use:

- `learning_episodes`;
- `skill_review_reports`;
- `skill_learning_events`;
- `skill_nudge_signals`;
- `skill_nudge_state`;
- `conversation_messages`;
- `execution_events`.

Episode detail should load selected message/execution refs from
`message_refs_json` and `execution_event_refs_json`. Evidence exposure follows
the current safety posture: do not expose low-trust message content as trusted
evidence, and do not expose thinking-only content as primary candidate
evidence.

Reports keep the current detail behavior and link back to the episode when
`skill_review_reports.learning_episode_id` is present.

### Usage

Reuse the aggregation semantics of `right_agent::usage::aggregate`.

The API returns structured data, not the Telegram HTML formatter output.
Malformed per-model JSON rows should be skipped with logging, matching current
aggregate behavior.

### Identity

Sandbox mode:

1. Try bounded live read of `/sandbox/IDENTITY.md`, `/sandbox/SOUL.md`, and
   `/sandbox/USER.md`.
2. If live sandbox read fails, return host mirror content from `agent_dir/`
   where available.
3. Include per-file source: `sandbox`, `host_mirror`, or `unavailable`.

No-sandbox mode:

- Read directly from `agent_dir/`.

### Skills

Sandbox mode:

1. Scan `/sandbox/.claude/skills/*/SKILL.md`.
2. If sandbox scan fails, scan `agent_dir/.claude/skills/*/SKILL.md`.
3. Read `installed.json` when present to enrich source metadata.

No-sandbox mode:

- Scan `agent_dir/.claude/skills/*/SKILL.md`.

Skill preview is bounded and should include the first useful `SKILL.md` content,
not every file in the package.

### Health

Doctor:

- Run `right_agent::doctor::run_doctor(home)` live on request.
- Return grouped structured checks.
- Do not invent doctor results when execution fails.

Sandbox:

- Use bounded OpenShell execution for cheap probes only.
- Collect disk free, CPU/RAM snapshot, and in-sandbox process list.
- Return unavailable sections when sandbox is not ready or probes fail.
- Do not expose process control.

## Frontend Design

The frontend remains Vue 3 + Vite + TypeScript.

Refactor the current single-app structure into small domain components before
adding more UI:

- app shell and navigation;
- overview view;
- activity view and run detail;
- knowledge learning views;
- knowledge skills views;
- usage view;
- identity view;
- health doctor/sandbox views;
- shared status, metric, list/detail, and error-state components.

Mobile behavior:

- single-column layout;
- list/detail drill-in;
- stable dimensions for status cards and rows;
- no dense nested cards.

Desktop behavior:

- list plus detail panel where useful;
- sticky detail panels only when they do not break mobile.

The UI should remain operational and restrained. It should not become a
marketing landing page.

## Auth And Security

Auth remains unchanged:

- every API request sends raw Telegram Mini App `initData` as
  `Authorization: tma <raw-init-data>`;
- backend validates HMAC with the agent bot token;
- `auth_date` freshness is enforced;
- user id must be trusted by allowlist;
- agent path must match the running bot.

Failure behavior:

- invalid/missing/expired Telegram auth returns `401`;
- valid auth from non-trusted users returns `403`;
- agent mismatch returns `403`;
- raw `initData` is never logged.

All new routes are read-only. The frontend must not talk directly to SQLite,
OpenShell, process-compose, the MCP aggregator, credentials, or files.

## Slash Commands

`/doctor` remains a slash command and keeps current behavior.

`/usage` is removed from Telegram command autocomplete. If a user manually
sends `/usage`, the bot should respond with a dashboard launch button or prompt
instead of rendering the old Telegram HTML usage report.

## Error Handling

- API errors use stable JSON error categories.
- Domain failures affect only the requested domain where practical.
- Missing optional logs, identity files, skill files, or sandbox snapshots are
  represented in response payloads instead of forcing the whole tab offline.
- Database/query failures are logged and returned as API errors; do not invent
  fake empty data.
- Live sandbox probe failures include bounded category/detail strings suitable
  for display.
- Payloads must be capped for mobile safety:
  - log lines;
  - evidence snippets;
  - identity bytes;
  - `SKILL.md` preview bytes;
  - process count;
  - process command length.

## Testing And Verification

Use TDD for behavior changes during implementation.

Focused Rust tests:

- DTO contract tests for new API shapes.
- Activity read-model tests for cron/background/foreground summaries and run
  detail missing-log behavior.
- Learning episode list/detail tests with linked reports and selected refs.
- Usage read-model tests matching current aggregate semantics.
- Skills grouping tests for core, learned, and other.
- Identity source/fallback tests with sandbox-reader fakes.
- Doctor route tests for auth, agent mismatch, success grouping, and failure.
- Sandbox route tests against fake probe output.
- `/usage` compatibility test: command still parses manual input and returns a
  dashboard prompt.

Frontend checks:

- TypeScript typecheck.
- Production build.

Final implementation verification must include:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Targeted tests should be used while iterating. Full workspace tests are the
final gate, not punctuation after every small edit.

## Architecture Docs

Implementation must re-read and update drifted docs for touched subsystems:

- `ARCHITECTURE.md` if dashboard route contracts or crate boundaries change;
- `docs/architecture/modules.md` for new dashboard modules;
- `docs/architecture/lifecycle.md` for dashboard route and slash-command flow;
- `docs/architecture/sandbox.md` if new sandbox probe behavior is added;
- `docs/architecture/sessions.md` if Activity changes run/session semantics.
