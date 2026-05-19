# Learning Episode Selector For Background Skill Review

## Goal

Replace single-turn background learned-skill review with an episode-based review
pipeline that captures multi-turn user feedback, tool execution evidence, and
model reasoning context before deciding whether a reusable `rightx-*` skill
candidate exists.

The motivating failure was a foreground workflow where the useful lesson lived
across several messages: the agent created file artifacts, the user corrected
where those artifacts belonged, the agent migrated them into Notion, and then
performed a follow-up retrieval. The old review gate reviewed an earlier
low-value turn, then skipped later high-value turns because of cooldown. It did
not preserve a pending review target and did not reason over the whole
correction/fix episode.

## Decisions

- Persist one domain object: `learning_episode`.
- Treat "window" as an implementation detail: a bounded selected context inside
  a `learning_episode`, not a separate persisted concept.
- Remove fixed review cooldown semantics. Cooldown must not discard evidence.
- Replace cooldown with immediate capture, short settle delay, de-duplication,
  concurrency limits, daily/cost budgets, and episode status.
- Add typed `execution_events` persisted from the Claude Code stream.
- Store thinking in `execution_events` as `event_kind = 'thinking'`, without FTS.
- Use a cheap configurable `episode_selector_model` to select episode bounds and
  refs from a Rust-built corpus. Do not hard-code a vendor/model name.
- Run the existing background reviewer after an episode is selected. The
  reviewer reads the selected episode and decides `nothing_to_learn`,
  `create_candidate`, or `update_candidate`.

## Non-Goals

- Do not let the background reviewer mutate skill files.
- Do not give the reviewer arbitrary database or log access.
- Do not implement a Hermes-style normal-agent fork with default tools.
- Do not add FTS over thinking or raw execution events in this stage.
- Do not make thinking sufficient evidence for a skill candidate.
- Do not implement full skill curator/archive behavior.

## Hermes Reference

The local Hermes checkout on branch `fix/nix-onnxruntime-darwin` at commit
`a1325241` implements background review by passing `messages_snapshot =
list(messages)` to a fresh `AIAgent` after a foreground response completes. The
review prompt says to review the conversation above. There is no dedicated
episode selection or expansion protocol in that checkout.

Because the Hermes reviewer is a normal `AIAgent`, it can inherit broad tool
access, including session search when available. Right Agent should not copy
that boundary. Right's reviewer remains report-only; context selection is done
by the platform and persisted for reproducibility.

The previously cited `agent/curator.py` and `tools/skill_usage.py` files are not
present in the local Hermes checkout.

## Data Sources

### `conversation_messages`

Existing user-facing transcript table. It remains the source of social and
dialogue context:

- routed user messages;
- assistant replies;
- unaddressed nearby group messages;
- `root_session_id` and `turn_id` linkage;
- timestamps and Telegram chat/thread identity.

Unaddressed group messages may be included in an episode only as low-trust
context. They are not instructions by themselves.

### `execution_events`

New typed stream evidence table. The bot already writes raw stream NDJSON to
`~/.right/logs/streams/<session-uuid>.ndjson`; this table normalizes the events
needed for review.

Shape:

```text
execution_events
  id
  agent_name
  root_session_id
  invocation_id
  turn_id
  async_run_id
  cron_job_name
  cron_run_id
  seq
  event_kind              -- assistant_text | thinking | tool_call |
                          -- tool_result | tool_error |
                          -- invocation_result | other
  tool_name
  content_json            -- bounded raw-ish event payload
  content_text            -- bounded compact summary/excerpt
  trust_label             -- primary | secondary | low_trust
  created_at
```

Rules:

- `thinking` is stored in this table, not in `conversation_messages`.
- `thinking` has no FTS.
- `thinking` is `secondary` context.
- tool calls, tool results, tool errors, assistant text, invocation results, and
  user/assistant transcript rows are primary observable evidence.
- A skill candidate must not cite only `thinking` refs.
- If thinking conflicts with transcript or tool evidence, observable evidence
  wins.

### Existing Learning Tables

The selector and reviewer also receive:

- `skill_nudge_signals`;
- `skill_learning_events`;
- current `rightx-*` skill index;
- existing `skill_review_reports` for de-duplication.

## New Domain Object

### `learning_episodes`

The selected reviewable unit.

Shape:

```text
learning_episodes
  id
  agent_name
  kind                    -- foreground_thread | async_continuation | cron_run
  seed_trigger_kind       -- learning_signal | skill_issue_signal |
                          -- effort_threshold | cron | async_result
  seed_ref
  status                  -- pending | selecting | selected | reviewing |
                          -- reviewed | no_episode | insufficient_context |
                          -- failed
  target_chat_id
  target_thread_id
  start_ref
  end_ref
  message_refs_json
  execution_event_refs_json
  selector_model
  selector_output_json
  boundary_rationale
  confidence              -- low | medium | high
  context_incomplete
  episode_hash
  ready_after
  last_evidence_at
  created_at
  updated_at
```

`skill_review_reports` should link to `learning_episode_id` while preserving
`source_invocation_id` for compatibility and diagnostics.

## Pipeline

```text
foreground / async / cron execution
        |
        v
conversation_messages + execution_events + signals
        |
        v
trigger capture
        |
        v
pending learning_episode seed
        |
        v
Rust prefilter builds bounded selector corpus
        |
        v
episode_selector_model selects refs and bounds
        |
        v
Rust validates refs and persists learning_episode
        |
        v
background reviewer reads selected episode
        |
        v
skill_review_reports
```

## Trigger And Scheduling

Triggers:

- accepted `learning_signal`;
- accepted `skill_issue_signal`;
- effort threshold;
- async result delivery;
- cron run completion;
- user feedback after an async/cron notification.

The old fixed 30-minute review cooldown is removed. A trigger is captured
immediately as a pending episode seed.

Scheduling safeguards:

- existing per-agent `review_running` concurrency gate, renamed only if the
  implementation plan deliberately replaces the current gate API;
- per-agent daily review count;
- per-agent cost budget;
- short settle delay after recent chat/thread activity;
- episode de-duplication by `episode_hash`;
- pending episode coalescing when new nearby evidence arrives before review.

Settle delay is not a cooldown. It waits for likely follow-up correction before
selection/review. It must not discard or reset the pending seed.

## Rust Prefilter

Rust builds a bounded selector corpus. It should use deterministic heuristics to
collect likely relevant evidence, but it should not make the final semantic
episode-boundary decision.

For `foreground_thread` seeds, include:

- nearby routed turns in the same Telegram chat/thread;
- unaddressed messages between routed turns, marked `low_trust`;
- assistant replies before and after the seed;
- execution events for included root sessions/turns;
- explicit learning signals/events;
- compact reasoning excerpts.

For `async_continuation` seeds, include:

- the foreground user intent that led to background handoff;
- foreground execution events before handoff;
- the `async_runs` row;
- execution events for `async_runs.run_session_id`;
- delivered result/notification;
- nearby user feedback after delivery in the target thread.

For `cron_run` seeds, include:

- the cron spec;
- current run execution events and notification/no-notify result;
- bounded recent prior runs of the same job;
- user feedback after a notification when present;
- the conversation turn that created or edited the cron when linked.

The corpus item format must enumerate stable refs. The selector can only choose
from those refs.

## Episode Selector Model

The selector is a cheap configurable model, referred to by config as
`episode_selector_model`.

Input:

- compact indexed corpus;
- seed trigger;
- budgets and trust labels;
- instruction to choose one reviewable learning episode, or no episode.

Output schema:

```json
{
  "status": "selected | no_episode | insufficient_context",
  "kind": "foreground_thread | async_continuation | cron_run",
  "start_ref": "msg:123",
  "end_ref": "msg:130",
  "message_refs": ["msg:123", "msg:124"],
  "execution_event_refs": ["exec:456", "exec:457"],
  "boundary_rationale": "The episode starts with the user's artifact-location correction and ends after the verified Notion migration.",
  "confidence": "low | medium | high",
  "context_incomplete": false
}
```

Boundary rules:

- Start at the earliest user intent that caused the workflow.
- Include prior failed or wrong assistant behavior when a later correction
  depends on it.
- Include the user correction that changed the approach.
- End after the verified successful outcome, delivered async result, explicit
  user confirmation, or first unrelated routed turn.
- Prefer contiguous evidence in the same thread/session lineage.
- Do not include unrelated turns just because they are near in time.

Rust validates:

- refs exist;
- refs are inside the corpus;
- refs do not exceed budgets;
- selected refs preserve trust labels;
- `start_ref` and `end_ref` are coherent for the selected kind;
- `thinking` refs are not the only selected evidence.

If validation fails, persist the episode as `failed` or
`insufficient_context`; do not run reviewer.

## Reviewer Input

The reviewer receives a full selected episode, not a delta and not an implicit
conversation continuation.

Sections:

- episode metadata;
- trusted and low-trust messages;
- execution events with tool calls/results/errors;
- thinking events as secondary context;
- learning signals/events;
- existing `rightx-*` skill index.

Reviewer rules:

- Report-only. No writes.
- `nothing_to_learn` is normal.
- Candidates must be reusable across future sessions.
- Candidate evidence must cite observable refs: messages, tool calls, tool
  results/errors, assistant final answers, or explicit signals.
- Thinking may improve wording and explain pitfalls, but it cannot be the only
  evidence for a candidate.
- Prefer update candidates for existing `rightx-*` skills.
- Do not preserve one-off task narrative.
- Do not make persistent negative claims from transient failures.

## Status Transitions

```text
pending
  -> selecting
  -> selected
  -> reviewing
  -> reviewed

pending/selecting
  -> no_episode
  -> insufficient_context
  -> failed
```

`insufficient_context` means the corpus did not contain enough bounded evidence
to select a reliable episode. A later trigger can create a new episode with a
richer corpus.

## De-Duplication

Compute `episode_hash` from:

- agent name;
- kind;
- selected message refs;
- selected execution event refs;
- seed trigger kind.

Before running reviewer, check for an existing reviewed or pending episode with
the same hash. Coalesce pending duplicates instead of spawning another review.

## Trust And Safety

- Wrap all user, assistant, stream, and thinking content as untrusted context.
- Preserve low-trust labels for unaddressed group messages.
- The selector and reviewer must not receive credentials or raw auth tokens.
- Tool inputs/results stored in `execution_events` must be bounded before
  insertion. The implementation must reuse the existing stream summarization
  helpers for tool inputs and add matching bounded summaries for tool results
  and errors before storing them.
- Do not expose mutation tools to the reviewer.
- Do not let selector output choose refs outside the Rust-provided corpus.

## Observability

Log and persist enough information to explain why a review did or did not run:

- trigger captured;
- settle delay scheduled;
- selector model and status;
- number of corpus messages/events;
- selected refs count;
- selector confidence and rationale;
- reviewer status/confidence/candidate;
- notification result.

Review reports that do not notify Telegram remain persisted. `nothing_to_learn`
stays silent for users by default.

## Testing Strategy

Unit tests:

- `execution_events` schema and insertion for text, thinking, tool call, tool
  result/error, and invocation result events.
- No FTS is created for `execution_events`.
- Thinking-only selector output is rejected for candidate review.
- Cooldown no longer drops eligible triggers.
- Pending seeds coalesce with nearby evidence before selection.
- Selector output validation rejects refs outside corpus.
- Episode hash de-duplicates selected episodes.

Integration tests:

- Multi-turn foreground correction becomes one `learning_episode`.
- Async background handoff includes source foreground session, background run,
  delivered result, and user feedback after delivery.
- Cron episode includes current run, bounded prior same-job runs, and linked
  notification feedback when present.
- `nothing_to_learn` report persists without Telegram notification.
- Reviewer receives thinking as secondary context and observable evidence as
  primary refs.

Final verification for implementation must include the full workspace test
suite.
