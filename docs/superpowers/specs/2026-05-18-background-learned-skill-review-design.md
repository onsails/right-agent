# Background Learned-Skill Review Design

## Goal

Ship Stage 2 of learned skills: a background, report-only reviewer that inspects
completed foreground turns and records reusable-skill candidates without
creating, patching, deleting, archiving, or moving skill files.

Stage 1 already lets the foreground agent create or update `rightx-*` learned
skills through `mcp__right__skill_learning_start` and
`mcp__right__skill_learning_finish`. Stage 2 adds the missing reliability loop:
if the foreground agent leaves a learning signal, a skill issue signal, or
enough tool effort accumulates, a separate background review invocation studies
the completed turn and stores a structured report.

## Hermes Reference

Hermes uses two related but distinct background patterns:

- Background review: a separate `AIAgent` runs against a bounded messages
  snapshot plus a review prompt. It is not a live continuation of the main
  provider session. It reuses model/provider configuration and cached system
  prompt state for prefix-cache efficiency while keeping the active conversation
  untouched.
- Curator: a separate `AIAgent` with `platform="curator"`,
  `skip_context_files=True`, and `skip_memory=True` reviews the skill library
  using skill usage telemetry. It can apply stale/archive/consolidation
  decisions.

Right Agent should adapt the background-review shape first. Curator behavior is
deferred because it needs mature usage telemetry, pins, snapshots, and rollback
policy. Stage 2 copies Hermes' effort-trigger idea for report-only review, but
does not copy automatic skill mutation.

Primary Hermes files examined during design:

- `agent/background_review.py`
- `agent/conversation_loop.py`
- `agent/curator.py`
- `tools/skill_manager_tool.py`
- `tools/skill_usage.py`

## Existing Right Agent Context

Implemented Stage 1 surfaces:

- Built-in authoring skill: `crates/right-codegen/skills/right-learn-skill/SKILL.md`
- Learned-skill MCP tools:
  - `mcp__right__skill_learning_start`
  - `mcp__right__skill_learning_finish`
- Domain/persistence helpers: `crates/right-agent/src/learned_skills.rs`
- Database schema: `crates/right-db/src/sql/v20_learned_skills.sql`
- Foreground reply schema fields:
  - `used_skill_receipts`
  - `learning_signal`
  - `skill_issue_signal`
- Reserved invocation kind: `ProgressInvocationKind::BackgroundReview`

Current constraints that Stage 2 must preserve:

- Only `rightx-*` learned skills are in scope.
- Custom/manual/hub/core/platform/bundled/codegen-owned skills are not mutated.
- Cron, reflection, delivery, and background-continuation paths deny
  foreground-only tools.
- Agents see Right MCP tools only with the `mcp__right__<tool>` prefix in
  prompts, docs, skills, and templates.
- Bot-first management remains the control plane for operational settings.

## Selected Approach

Use "report-only + telemetry foundation":

1. Trigger background review from foreground completion when either:
   - a valid `learning_signal` exists;
   - a valid `skill_issue_signal` exists;
   - `tool_iters_since_review >= creation_review_interval`.
2. Start a separate background Claude Code invocation with a bounded review
   bundle.
3. Require structured report output.
4. Store every report.
5. Send Telegram only for high-confidence create/update candidates.
6. Do not write skill package files in Stage 2.

This approach lets Right Agent measure review quality before giving background
workers mutation authority.

## Non-Goals

Stage 2 does not:

- create skill package files;
- patch existing skill package files;
- call `mcp__right__skill_learning_start`;
- call `mcp__right__skill_learning_finish`;
- archive, delete, consolidate, or pin skills;
- implement Hermes Curator parity;
- implement GEPA or offline prompt evolution;
- expose user approval flows for drafts;
- add broad marketplace or hub management changes.

## Architecture

Foreground completion remains the scheduling point.

Flow:

1. Foreground turn finishes and reply metadata is parsed.
2. Bot persists existing learned-skill metadata:
   - `skill_learning_events`
   - `skill_nudge_signals`
   - `skill_nudge_state`
   - `used_skill_receipts` when present
3. A review scheduler evaluates gates.
4. If eligible, scheduler marks `review_running = true`.
5. Scheduler starts a separate `BackgroundReview` Claude Code invocation.
6. The background invocation receives a bounded review bundle, not live session
   continuation.
7. The reviewer returns structured output:
   - `nothing_to_learn`
   - `create_candidate`
   - `update_candidate`
   - `failed`
8. Bot stores the report and updates counters.
9. Bot sends a Telegram notice only for high-confidence candidates.

The background reviewer is intentionally a new invocation. It should not resume
or fork a live Claude Code session. It receives enough context to review the
completed turn and nothing more.

## Gates

Eligibility:

```text
eligible if:
  review_running = false
AND daily_review_count < daily_limit
AND now - last_review_at >= cooldown
AND (
  accepted learning_signal exists
  OR accepted skill_issue_signal exists
  OR tool_iters_since_review >= creation_review_interval
)
```

Defaults:

```text
creation_review_interval = 15
cooldown = 30 minutes
daily_limit = 12 per agent
```

Signal-triggered reviews bypass `creation_review_interval`, but do not bypass
concurrency, cooldown, or daily limit in Stage 2. A later feature can let
explicit user-request signals bypass cooldown if real usage shows that is
needed.

After a successful review:

- set `review_running = false`;
- set `last_review_at = now`;
- reset `tool_iters_since_review = 0`;
- reset `turns_since_review = 0`;
- reset `skill_issue_hints_since_review` when the review addresses a skill
  issue or returns `nothing_to_learn`.

After a failed review:

- set `review_running = false`;
- store `status = failed`;
- do not reset effort counters;
- apply normal cooldown before retry.

No cron-style periodic scan is included in Stage 2.

## Data Model

Add table `skill_review_reports`:

```text
id
agent_name
source_invocation_id
root_session_id
chat_id
thread_id
trigger_kind              -- learning_signal | skill_issue_signal | effort_threshold
status                    -- nothing_to_learn | create_candidate | update_candidate | failed
confidence                -- low | medium | high
candidate_skill_name      -- nullable, rightx-* when present
candidate_summary
evidence_refs_json
review_output_json
telegram_notified         -- integer bool
created_at
```

Extend `skill_nudge_state`:

```text
creation_review_interval  -- default 15
daily_review_count
daily_review_date
last_review_status
```

Do not add full Hermes-style skill usage parity in Stage 2. Review telemetry and
existing `used_skill_receipts` are enough to evaluate the background reviewer.
Explicit per-skill usage tables can be added in a later curator-focused stage.

## Review Bundle

The bundle must be bounded and deterministic. It must not be "last N log
lines".

Inputs:

- `source_invocation_id`
- `agent_name`
- `root_session_id`
- trigger kind
- accepted signal payload when present
- tool iteration count and nudge counters
- compact event timeline:
  - user messages
  - assistant final answer
  - tool call/result summaries
  - tool errors
  - retries when detectable
  - verification steps when detectable
- current learned skill index:
  - only `rightx-*`
  - skill name
  - description/frontmatter
  - full `SKILL.md` only when small, otherwise a bounded excerpt
- existing learning events for the same invocation
- report-only reviewer instructions

Reviewer instructions must say:

- no writes;
- no skill learning tools;
- `nothing_to_learn` is normal;
- candidates must be reusable across future sessions;
- do not preserve one-off task narrative;
- do not make persistent negative claims from transient tool failures;
- prefer update candidates for existing `rightx-*` skills when applicable.

If stable stream event ids exist, use them. Otherwise derive invocation-local
ids while building the bundle.

## Invocation Boundary

The background review invocation should enforce report-only behavior through
tool restrictions, not only through prompt text.

Disallow:

- `mcp__right__send_progress`
- `mcp__right__skill_learning_start`
- `mcp__right__skill_learning_finish`
- `Agent`

The reviewer should not receive tool access that can mutate project files or
skill package files. If the current invocation builder cannot enforce a
read-only filesystem surface, Stage 2 should feed the review bundle as the main
input and avoid exposing the working tree as a writable target.

`ProgressInvocationKind::BackgroundReview` remains useful as a typed execution
kind, but Stage 2 should not expose learning start/finish tools to it. Later
stages can relax this after report quality is proven.

## Structured Output

Review output schema:

```json
{
  "status": "nothing_to_learn | create_candidate | update_candidate | failed",
  "confidence": "low | medium | high",
  "candidate_skill_name": "rightx-example-or-null",
  "candidate_summary": "short reusable lesson summary or null",
  "evidence_refs": ["event-1"],
  "user_notice": "short Telegram notice only for high-confidence candidates"
}
```

Validation:

- `create_candidate` and `update_candidate` require non-empty evidence refs.
- Candidate skill names must start with `rightx-` when present.
- `nothing_to_learn` may have empty candidate fields.
- `failed` must preserve enough output to debug the failure.

Telegram notice rule:

```text
send notice if:
  status in create_candidate/update_candidate
AND confidence = high
AND user_notice is non-empty
```

The notice must not include generated skill content. It should say only that a
reusable workflow candidate was recorded for review.

## Error Handling

- Review spawn failure records a failed report when possible and clears
  `review_running`.
- Invalid structured output records `status = failed`.
- Telegram notice failure does not invalidate the stored report.
- If the source invocation bundle cannot be built, no review is spawned and the
  counters remain for a later eligible turn.
- A background review must never trigger another background review.

## Security

Stage 2 treats review as observation only.

Security constraints:

- No skill file writes.
- No project file writes.
- No mutation MCP tools.
- No recursive agent spawning.
- No user secrets in review bundle.
- Chat/thread ids are internal routing metadata and should not be exposed in
  reviewer prompt unless needed for debugging.
- Only `rightx-*` skills are indexed.

## Testing

Implementation should follow TDD.

Test groups:

1. Database migration tests
   - `skill_review_reports` exists.
   - extended `skill_nudge_state` defaults are usable.
   - daily counters reset by date logic.

2. Gate tests
   - signal triggers review.
   - effort threshold triggers review.
   - cooldown blocks review.
   - `review_running` blocks review.
   - daily limit blocks review.
   - failed review clears `review_running`.

3. Bundle tests
   - includes accepted signal.
   - includes only `rightx-*` skill index.
   - excludes custom/manual/bundled skills.
   - event refs are stable and non-empty.
   - large transcript is bounded.

4. Invocation tests
   - background review disallows foreground-only tools.
   - `Agent` is disallowed.
   - report-only schema parses `nothing_to_learn`.
   - high-confidence candidate records report and sends notice.
   - low/medium candidate records report but sends no Telegram notice.

5. Integration-style test
   - simulate foreground turn with `learning_signal`;
   - scheduler starts background review;
   - mocked Claude Code returns high-confidence `create_candidate`;
   - DB report is stored;
   - Telegram notice is queued or sent;
   - no skill package files are written.

Final verification:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

## Open Decisions For Implementation Plan

- Exact module location for the scheduler and bundle builder.
- Whether the first implementation should mock Claude Code at the invocation
  layer or add a smaller review-runner abstraction.
- Exact wording of the reviewer system prompt.
- Whether `creation_review_interval`, cooldown, and daily limit are hard-coded
  constants in Stage 2 or exposed through bot-managed config later.

These are implementation-plan decisions, not design blockers.
