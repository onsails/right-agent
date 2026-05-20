# Dashboard Learned-Skill Metrics Design

## Goal

Extend the Telegram Mini App dashboard with read-only learned-skill metrics that
explain why useful `rightx-*` skills are or are not being created.

The primary user question is:

```text
Why is the agent not creating useful learned skills?
```

The dashboard should answer that with a learning funnel first, then supporting
quality, health, lifecycle, and evidence views.

## Current System Context

Right Agent has two learned-skill paths:

- Stage 1 foreground learning: the active agent may create or update only
  `rightx-*` skills through `/right-learn-skill` and
  `mcp__right__skill_learning_start` /
  `mcp__right__skill_learning_finish`.
- Stage 2 background review: report-only selector/reviewer invocations inspect
  durable `learning_episodes`, run with no MCP/tools, and write
  `skill_review_reports`. They never create, patch, archive, or delete skill
  package files.

The current dashboard on `master` is cron-centered. It exposes cron/runs/cost
read models but does not read learned-skill tables:

- `skill_nudge_signals`
- `learning_episodes`
- `skill_review_reports`
- `skill_learning_events`
- `execution_events`
- `conversation_messages`

## Selected Approach

Add a separate learning read model and API route group instead of expanding the
cron overview payload.

Reasons:

- Learning evidence snippets should not be loaded on every cron poll.
- Cron concepts and learning-review concepts have different lifecycles.
- A separate API surface gives the frontend explicit capabilities for showing
  or hiding learning UI controls.
- The boundary leaves room for future write operations without making the Vue
  app talk directly to databases, files, MCP, or process-compose.

## Scope

In scope:

- A new Learning view/tab in the existing Telegram Mini App dashboard.
- Read-only API routes for learning overview and report detail.
- Learning funnel metrics over a short rolling window.
- Quality and health counters.
- Lifecycle summary from foreground learning events.
- Bounded evidence snippets for selected reports.
- Backend-provided capabilities so the frontend does not render unavailable
  buttons or tabs.

Out of scope:

- Candidate approval, retry, create, edit, archive, or delete operations.
- Raw NDJSON log viewing.
- Full reviewer prompt/output viewing.
- Chat-scoped evidence authorization.
- Exposing thinking or low-trust context in the dashboard.
- A separate dashboard process.

## Backend Architecture

`right-dashboard` remains the read-model and DTO crate.

Add DTOs in `right-dashboard::api_types` for:

- `LearningOverviewResponse`
- `LearningCapabilities`
- `LearningFunnel`
- `LearningQuality`
- `LearningHealth`
- `LearningLifecycle`
- `LearningReportSummary`
- `LearningReportDetailResponse`
- `LearningEvidenceSnippet`

Add query functions under `right-dashboard::read_model`. If
`read_model.rs` becomes too large, split the internal implementation into a
learning submodule while keeping the public API stable.

`right-bot::telegram::dashboard` owns route mounting and auth, same as the
existing cron dashboard routes.

New authenticated routes:

```text
GET /dashboard/<agent>/api/v1/learning/overview
GET /dashboard/<agent>/api/v1/learning/reports/{report_id}
```

Existing Telegram Mini App auth remains unchanged:

- raw Telegram `initData` in `Authorization: tma <raw-init-data>`;
- HMAC validation with the agent bot token;
- `auth_date` freshness check;
- allowlist authorization;
- agent path must match the running bot.

## Capabilities Contract

The backend must tell the frontend which learning UI surfaces are available.
The frontend must hide unavailable UI instead of rendering dead disabled
buttons.

Extend `bootstrap.features` or add an equivalent capabilities object with:

```text
learning_metrics: bool
learning_evidence_snippets: bool
learning_commands: bool
```

For this spec:

```text
learning_metrics = true
learning_evidence_snippets = true
learning_commands = false
```

Future write capabilities such as `approve_candidates`, `retry_review`, or
`disable_skill` must be separate booleans and require a separate design before
routes are exposed.

## Overview API

`GET /learning/overview` returns a compact polling payload:

```text
LearningOverviewResponse
- agent
- generated_at
- refresh_interval_secs
- capabilities
- funnel
- quality
- health
- lifecycle
- recent_reports
```

Use these default windows:

- Main funnel: last 24 hours.
- Recent reports: latest 20 reports.
- Lifecycle summary: last 7 days.

### Funnel

The funnel answers where learning attempts are dropping:

```text
signals_accepted_24h
episodes_pending_24h
episodes_selecting_24h
episodes_selected_24h
episodes_reviewing_24h
episodes_reviewed_24h
episodes_no_episode_24h
episodes_insufficient_context_24h
episodes_failed_24h
reports_total_24h
create_candidates_24h
update_candidates_24h
nothing_to_learn_24h
failed_reviews_24h
foreground_created_or_updated_7d
```

Definitions:

- `signals_accepted_24h`: rows in `skill_nudge_signals`.
- `episodes_*_24h`: rows in `learning_episodes` by `status`.
- `reports_total_24h`: rows in `skill_review_reports`.
- `create_candidates_24h`: reports with `status='create_candidate'`.
- `update_candidates_24h`: reports with `status='update_candidate'`.
- `nothing_to_learn_24h`: reports with `status='nothing_to_learn'`.
- `failed_reviews_24h`: reports with `status='failed'`.
- `foreground_created_or_updated_7d`: successful `skill_learning_events`
  finish rows with `status IN ('created','updated')`.

### Quality

Quality metrics should explain reviewer behavior:

```text
candidate_rate
nothing_to_learn_rate
create_count_24h
update_count_24h
high_confidence_count_24h
medium_confidence_count_24h
low_confidence_count_24h
failed_count_24h
```

Rates use reviewed, non-failed reports as the denominator:

```text
candidate_rate = (create_candidate + update_candidate) / non_failed_reports
nothing_to_learn_rate = nothing_to_learn / non_failed_reports
```

If the denominator is zero, rates are `null`, not `0.0`.

### Health

Health is current state from `skill_nudge_state` plus derived stuck indicators:

```text
review_running
daily_review_count
daily_limit
creation_review_interval
tool_iters_since_review
turns_since_review
skill_issue_hints_since_review
last_review_status
last_review_at
possibly_stuck
```

Stuck detection is derived:

- Prefer `learning_episodes.status='reviewing'` with old `updated_at`.
- Use `skill_nudge_state.review_running` as a broad gate signal.
- Do not invent a stuck reason if timestamps are missing; expose
  `possibly_stuck=false` and leave timestamps null.

### Lifecycle

Lifecycle should connect report-only candidates to actual foreground learning:

```text
created_7d
updated_7d
failed_or_aborted_7d
recent_successful_events
candidate_skill_names_7d
```

`recent_successful_events` comes from `skill_learning_events` finish rows with
`status IN ('created','updated')`.

`candidate_skill_names_7d` comes from `skill_review_reports` candidate rows and
is a bounded list, not a live filesystem scan.

## Report Detail API

`GET /learning/reports/{report_id}` returns one report and bounded evidence:

```text
LearningReportDetailResponse
- report
- episode
- selector
- evidence
- reviewer
```

Report fields:

- id
- status
- confidence
- trigger kind
- candidate skill name
- candidate summary
- Telegram-notified flag
- created timestamp

Episode fields:

- learning episode id
- kind
- seed trigger kind
- status
- start/end refs
- boundary rationale
- selector confidence
- context-incomplete flag

Selector fields:

- model if stored
- boundary rationale
- selected message refs
- selected execution event refs

Reviewer fields:

- status
- confidence
- candidate name
- candidate summary
- evidence refs
- user notice presence

Do not return full raw reviewer prompt or full raw output in this spec.

## Evidence Snippets

Report detail returns short snippets for selected refs only:

- `msg:*` refs from `conversation_messages`;
- non-thinking `exec:*` refs from `execution_events`;
- enough metadata to identify kind, trust label, role/tool, and timestamp;
- bounded text content.

Snippet DTO:

```text
LearningEvidenceSnippet
- ref_id
- source: message | execution_event
- available
- trust_label
- role
- event_kind
- tool_name
- created_at
- text
```

Rules:

- Missing refs become `available=false` snippets, not a failed detail response.
- Snippet text is capped.
- Raw logs are not returned.
- Thinking events are not returned.
- Low-trust messages are not returned.
- Secrets are not newly redacted here; evidence comes from already persisted
  redacted execution events and archived messages. The response must still
  avoid returning raw JSON blobs.

## Frontend Design

Add a top-level view switch in the existing Vue app:

```text
Cron | Learning
```

The Learning tab appears only when backend capabilities allow
`learning_metrics`.

Learning view layout:

1. Funnel strip:
   `signals -> episodes -> selected -> reviewed -> candidates -> created/updated`
2. Main report list:
   latest reports with status, confidence, trigger, candidate name, and short
   summary.
3. Side panel:
   quality ratios and operational health counters.
4. Detail panel:
   fetched on report selection; shows selector rationale, reviewer summary, and
   bounded evidence snippets.

No mutation controls are rendered in this spec because
`learning_commands=false`.

## Error Handling

API errors use stable JSON categories:

- overview query failure: `500 learning_overview_failed`;
- report query failure: `500 learning_report_failed`;
- missing report: `404 not_found`;
- unauthorized or forbidden: existing dashboard auth errors;
- agent mismatch: existing dashboard agent mismatch behavior.

Missing evidence refs are represented in the response with `available=false`.

Malformed stored JSON in selector/reviewer fields returns `500`. This is
intentional: partial fabricated data would hide database corruption or parser
drift.

## Security

Evidence snippets are visible to all allowlisted dashboard users for the agent.
There is no chat-scoped authorization in this spec.

Security constraints:

- Validate Telegram Mini App auth on every API request.
- Never log raw `initData`.
- Never return raw prompts, raw NDJSON logs, full reviewer stdout, or full
  reviewer prompt bundles.
- Do not expose write routes.
- Do not expose MCP, aggregator, process-compose, or file mutation through Vue.
- Bound all evidence payloads.
- Do not show controls unless backend capabilities say they are available.

## Future Mutations

Future dashboard writes must remain bot-owned command routes:

- explicit `POST` endpoints;
- typed request/result DTOs;
- stricter auth/freshness than read routes;
- operation-specific policy checks;
- audit/event logging;
- no direct `.mcp.json`, credential, skill file, or `agent.yaml` edits from
  Vue.

Candidate approval is intentionally deferred. The first future write should get
its own design because it changes the Stage 2 report-only boundary.

## Testing And Verification

Use TDD when implementing.

Focused tests:

- `right-dashboard` read model builds learning funnel from fixture signals,
  episodes, reports, and learning events.
- Candidate and nothing-to-learn rates use the correct denominator and return
  `null` when there are no reviewed non-failed reports.
- Report detail returns bounded snippets for selected message and execution refs.
- Report detail marks missing refs as `available=false`.
- Report detail excludes thinking and low-trust snippets.
- Malformed stored selector/reviewer JSON returns a query error.
- `right-bot` route tests cover auth, overview success, detail success,
  missing detail `404`, and capabilities in bootstrap.
- Frontend types match backend DTOs.
- Frontend hides Learning when `learning_metrics=false`.
- Frontend hides command buttons when `learning_commands=false`.

Final verification:

```bash
devenv shell -- cargo test -p right-dashboard learning
devenv shell -- cargo test -p right-bot dashboard
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```
