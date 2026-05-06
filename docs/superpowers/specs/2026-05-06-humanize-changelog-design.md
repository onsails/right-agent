# Humanize the release-plz Changelog

## Problem

`release-plz` regenerates `CHANGELOG.md` from `cliff.toml` on every push to
master, force-pushing the release PR. The output is a grouped list of raw
commit subjects:

```
### Bug Fixes
- **cron**: Read delivery target from cron_runs, drop JOIN to cron_specs
- **openshell**: Restore ssh_exec cancel-safety via RAII pid guard
### Features
- **cron**: Add ScheduleKind::Immediate variant with @immediate sentinel
### Refactor
- **cron**: Extract reconcile predicate fns so regression tests bind to production
```

This is mechanism-first noise for an audience that wants user-visible
consequence. Half the entries in a typical release are pure-internal
(test refactors, review-loop fixups, schema-version bumps) and teach an
operator nothing.

The repo already has the Claude GitHub App installed and uses
`anthropics/claude-code-action@v1` in two workflows. Use it to humanize
the new section of `CHANGELOG.md` automatically on every release-plz PR
update, gated by a required check on master.

## Goals

- Replace the cliff-generated section of every release-plz PR with 3-7
  plain-English bullets describing what an operator running `right` would
  notice in this release.
- Run automatically on PR `opened`/`synchronize`/`reopened`/`ready_for_review`
  so release-plz force-pushes are re-humanized.
- Block the release PR from merging while the humanize step is running or
  has failed (hard fail). Polish is non-negotiable.
- Stay inside existing repo conventions: same action, same secret, same
  prompt-file pattern as `claude-code-review.yml`.

## Non-goals

- A second copy of the technical changelog. `git log` is the technical
  record; CHANGELOG.md is the operator-facing record.
- A PR-time `cargo check` / `cargo test` gate. Worth doing — out of scope
  here. Branch protection in this spec requires only the humanize check.
- Preserving cliff's grouping (`### Features`, `### Bug Fixes`). The
  rewrite drops it.
- A `workflow_dispatch` re-trigger. Empty commit to master is the
  re-trigger path.

## Decisions

| Question | Answer | Reasoning |
|---|---|---|
| Mechanism: cliff `postprocessors` + `claude -p`, or `claude-code-action`? | `claude-code-action` | Decouples release stability from LLM availability — a Claude blip during a postprocessor crashes release-plz and blocks the PR from being created at all. Action runs after the PR exists, so the cliff version is always a fallback. |
| Trigger: auto on PR open + every push, or `@claude` mention? | Auto on `opened` + `synchronize` + `reopened` + `ready_for_review` | release-plz force-pushes the release PR on every master push; humanize must re-run on each. |
| Shape: highlights + technical appendix, or replace cliff entirely? | Replace cliff entirely | Audience is operators, not contributors. Contributors have `git log`. Half the cliff entries are internal noise to operators anyway. |
| Context: pre-load commits or let Claude explore? | Claude explores via Bash, scoped to `git describe --tags --abbrev=0 --match='v*' HEAD^..HEAD` | claude-code-action gives Claude a checked-out repo and tools; pre-loading fights the action's design. Bounding scope keeps it from straying into earlier releases. |
| Failure mode: soft fail (PR keeps cliff output) or hard fail (PR blocked)? | Hard fail | Operator chose: shipping the mechanical version "by accident" is the wrong default. |
| Auth: `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`? | `CLAUDE_CODE_OAUTH_TOKEN` | Existing repo convention; already configured. Couples release availability to Max subscription quota — accepted given hard-fail. |
| Prompt location: inline YAML, separate file, or plugin? | Separate file `.github/prompts/humanize-changelog.md` | Multiline prose in YAML is awkward to maintain and to review in PR diffs. |
| Branch protection scope: humanize-only, humanize+build, or defer? | Humanize-only | Build (`build.yml`) is a release-event trigger, not a PR check. Coupling them requires introducing a real PR-time `cargo check` first — separate spec. |

## Artifacts

Three new files plus one one-time `gh api` call.

| File | Purpose |
|---|---|
| `.github/workflows/humanize-changelog.yml` | Triggers on release-plz PR events. Filters to release-plz-authored "chore: release" PRs. Loop-guards out Claude's own commits. Calls `anthropics/claude-code-action@v1` with the prompt file. |
| `.github/prompts/humanize-changelog.md` | The humanize prompt: scope, output shape, voice rules with before/after examples, drop list, commit message format. |
| `.github/rulesets/master.json` | GitHub Repository Ruleset. Targets the default branch. Requires the humanize check. Admin-bypass `always`. Committed for change tracking; applied via `gh api` once. |

## End-to-end flow

1. Push to master → existing `release-plz-pr` job opens or force-pushes the
   release PR titled `chore: release vX.Y.Z`, authored by
   `github-actions[bot]`.
2. PR `opened` or `synchronize` → `humanize-changelog.yml` fires.
3. Filter (job-level `if:`):
   - `startsWith(pull_request.title, 'chore: release')`
   - AND `pull_request.user.login == 'github-actions[bot]'`
   - AND `event.sender.login != 'claude[bot]'` (loop guard)
4. claude-code-action runs the prompt. Claude computes the version range
   from `git describe --tags --abbrev=0 --match='v*' HEAD^..HEAD`, reads
   commit messages and (when needed) diffs via Bash, rewrites the topmost
   `## [X.Y.Z] - YYYY-MM-DD` section in CHANGELOG.md, commits via the
   GitHub App's installation token.
5. Claude's push triggers another `synchronize` → loop guard skips it.
6. Required-checks gate keeps the PR unmergeable until the humanize check
   passes.
7. On Claude failure (rate limit, model overload, network), the workflow
   exits non-zero → check fails → admin bypass needed to ship.

## The workflow

```yaml
name: Humanize Changelog

on:
  pull_request:
    types: [opened, synchronize, reopened, ready_for_review]

concurrency:
  group: humanize-changelog-${{ github.event.pull_request.number }}
  cancel-in-progress: true

jobs:
  humanize:
    name: humanize
    if: |
      startsWith(github.event.pull_request.title, 'chore: release') &&
      github.event.pull_request.user.login == 'github-actions[bot]' &&
      github.event.sender.login != 'claude[bot]'
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
      id-token: write
      actions: read
    steps:
      - name: Checkout PR head
        uses: actions/checkout@v6
        with:
          ref: ${{ github.event.pull_request.head.ref }}
          fetch-depth: 0
          persist-credentials: false

      - name: Run Claude humanize
        uses: anthropics/claude-code-action@v1
        with:
          claude_code_oauth_token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
          prompt_file: .github/prompts/humanize-changelog.md
```

Mechanics worth noting:

- **`synchronize` is the critical event** — fires on every release-plz
  force-push.
- **Three-clause filter.** Title prefix narrows to release PRs.
  `user.login == 'github-actions[bot]'` distinguishes from a human PR
  titled "chore: release something else". `sender.login != 'claude[bot]'`
  is the loop guard.
- **Concurrency cancel-in-progress.** release-plz can force-push several
  times per minute during active master pushes. Without cancellation,
  parallel humanize runs race to push to the same branch.
- **Checkout `head.ref`, not the merge ref.** Default `actions/checkout`
  on `pull_request` checks out a synthetic merge commit, useless for
  committing back. We need the head branch directly.
- **`persist-credentials: false`.** Matches `release-plz.yml`.
  claude-code-action authenticates and pushes via the GitHub App's
  installation token, independent of the runner's `GITHUB_TOKEN`.
- **Same-repo only.** release-plz PRs always run from the repo itself,
  so secrets are reachable.

## The prompt

`.github/prompts/humanize-changelog.md`:

```markdown
# Humanize the latest CHANGELOG.md section

You are editing `CHANGELOG.md` in the **Right Agent** repo. Right Agent
is an opinionated, closed-box AI agent platform — operators run `right`
to spin up Telegram-driven Claude Code agents in OpenShell sandboxes.
The audience for this changelog is **those operators**, not contributors
browsing git history. They want to know what they will notice in this
release.

## Scope

The release-plz PR has just regenerated the topmost version section in
`CHANGELOG.md` (under `## [X.Y.Z] - YYYY-MM-DD`). **That is the only
section you rewrite.** Do not touch any earlier version section.

The commits in scope are exactly those between the previous `v*` tag
and HEAD:

    range="$(git describe --tags --abbrev=0 --match='v*' HEAD^)..HEAD"
    git log "$range" --no-merges --format='%H%n%s%n%b%n----'

Run `git show <sha>` if a commit's subject and body don't tell you
enough about user impact.

## Output shape

Replace the body of the topmost version section — everything after the
`## [X.Y.Z] - YYYY-MM-DD` heading line until the next `## [` heading or
end of file — with **3-7 plain markdown bullets**.

Keep the heading line `## [X.Y.Z] - YYYY-MM-DD` exactly as cliff
produced it. Do not change the version number or date.

If after dropping internal noise nothing operator-visible remains,
write one line instead:
`_Internal-only release. No operator-visible changes._`

## Voice

**Lead with user-visible consequence, not mechanism.**

| Don't                                                          | Do                                                                                                                     |
|----------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------|
| Drop JOIN to cron_specs in cron_runs delivery query            | Cron deliveries no longer get sent to the wrong chat when an agent's Telegram thread changes between schedule and run  |
| Restore ssh_exec cancel-safety via RAII pid guard              | Cancelling an in-flight agent command no longer leaves zombie ssh processes inside the sandbox                         |
| Address review-loop findings on background-continuation        | (drop — internal review pass, no operator-visible change)                                                              |

**Drop pure-internal entries** — anything an operator cannot observe:
- test additions and refactors
- internal renames, file moves, code reorganization
- review-loop / clippy / lint fixups
- schema-version bumps with no behavior change
- changes to dev-only or test-only code paths

**Group related commits.** Five commits implementing one feature get
**one** bullet. The reader does not care that the author split work
for review.

**Mark breaking changes** with a leading `**Breaking:**`. A commit is
breaking if it has `!` in the conventional-commit type (e.g. `feat!:`)
or a `BREAKING CHANGE:` trailer.

**Plain markdown bullets only.** No emojis. No `### Features` /
`### Bug Fixes` subgroups (those are the cliff output we are replacing).
No bold/italic except `**Breaking:**`. No nested bullets.

**Present tense, active voice.** "Cron retries failed deliveries." Not
"will now retry" or "are retried."

## Commit

Edit `CHANGELOG.md` in place. Stage and commit it with this message
exactly:

    chore(changelog): humanize v<VERSION>

where `<VERSION>` is the version number from the section's heading.

If the topmost section already looks humanized (e.g. release-plz did
not actually regenerate it on this trigger), still rewrite from scratch
from the commits in range. Output is deterministic from the commits,
not from the file's current contents.
```

Design choices in this prompt:

- **Few-shot table with three rows**, including one "drop entirely"
  case. Voice rules in the abstract get partially followed; concrete
  before/after examples drawn from real `0.2.9` commits anchor it.
- **Empty-release escape hatch** lets Claude refuse to invent. Without
  it, an LLM under pressure to produce 3-7 bullets will hallucinate.
- **"Rewrite from commits, not from the file"** defends against
  degenerate behavior on re-runs. Idempotency comes from the commit
  range, not from inspecting prior runs.
- **Audience framing in the first paragraph.** Without "operators
  running `right`", Claude doesn't know the threshold for
  "user-visible".

## The ruleset

`.github/rulesets/master.json`:

```json
{
  "name": "master-required-checks",
  "target": "branch",
  "enforcement": "active",
  "conditions": {
    "ref_name": {
      "include": ["~DEFAULT_BRANCH"],
      "exclude": []
    }
  },
  "bypass_actors": [
    { "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always" }
  ],
  "rules": [
    {
      "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 0,
        "dismiss_stale_reviews_on_push": false,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": false
      }
    },
    {
      "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [
          { "context": "humanize" }
        ]
      }
    }
  ]
}
```

Apply once after the workflow PR merges:

```bash
gh api -X POST /repos/onsails/right-agent/rulesets \
  -H "Accept: application/vnd.github+json" \
  --input .github/rulesets/master.json
```

Subsequent edits use `PUT /repos/.../rulesets/{id}` against the `id`
returned by the create call.

`bypass_mode: always` lets repo admins merge directly when Claude is
unavailable during a critical release. Switch to `pull_request` if
deliberate friction is preferred.

## Bootstrap order

1. Open a PR that adds:
   - `.github/workflows/humanize-changelog.yml`
   - `.github/prompts/humanize-changelog.md`
   - `.github/rulesets/master.json`
   - This spec doc
2. Merge the PR.
3. **Wait for the next release-plz PR to trigger humanize once.** Read
   the workflow run's `github.event.sender.login` from a real Claude
   commit to confirm the loop-guard string is `claude[bot]`. Read the
   check-run's `name` from
   `gh api repos/onsails/right-agent/commits/<sha>/check-runs` to
   confirm the ruleset's `context` value is `humanize`.
4. Adjust the workflow YAML and ruleset JSON if either telemetry
   disagrees with the spec.
5. Apply the ruleset:

   ```bash
   gh api -X POST /repos/onsails/right-agent/rulesets \
     -H "Accept: application/vnd.github+json" \
     --input .github/rulesets/master.json
   ```

The deliberate one-cycle delay before applying the ruleset prevents
a two-bug situation where a wrong loop-guard string causes Claude to
self-loop AND the ruleset blocks merges at the same time.

## Failure mode matrix

| Failure | Effect | Resolution |
|---|---|---|
| Claude rate-limited / Max quota hit | Workflow exits non-zero → required check fails → PR unmergeable | Wait for quota; or admin-bypass; or push empty commit to master to retrigger release-plz force-push → fresh humanize attempt |
| Claude produces semantically bad output (hallucination, dropped bullet) | Check passes (action succeeded). Content is bad but committed | Eyeball before merge. Empty commit to master → re-humanize. Or `@claude` mention via existing `claude.yml` for in-place tweaks |
| Claude breaks file structure (e.g. nukes the heading line) | Check passes but CHANGELOG.md is malformed | Same as above. Could add a structural sanity step (`grep -q '^## \[' CHANGELOG.md`) — declined as YAML complexity for an edge case |
| Loop guard misfires | Claude triggers itself, eats quota in a loop | One live-run check during bootstrap. Defer ruleset apply until confirmed |
| release-plz force-pushes during in-flight humanize | `cancel-in-progress` cancels the running job. Cancelled = not "success" → PR stays unmergeable until next humanize completes | Self-resolves when the latest force-push's humanize completes |
| Cliff template changes section format | Prompt anchored to `## [X.Y.Z] - YYYY-MM-DD` heading — survives unless cliff changes that shape | Re-test humanize on next release after any `cliff.toml` edit |
| Master pushes faster than humanize completes | Each force-push cancels the prior humanize. PR may stay "cancelled" through a rapid push burst | Self-resolves when master quiets. Acceptable: release-plz force-pushes are version-bump-triggered, not arbitrary |

## Re-trigger paths

- **Cheap and reliable:** push an empty commit to master:
  `git commit --allow-empty -m "chore: trigger release-plz" && git push`.
  release-plz force-pushes the release PR; humanize re-runs on
  `synchronize`.
- **In-place tweak via existing `claude.yml`:** `@claude` mention with a
  specific instruction. Different prompt and commit message — useful
  for one-bullet edits, not equivalent to a full re-humanize.
- **No `workflow_dispatch:`** in this design. Adding it requires
  building `pull_request` context manually from a `pr_number` input,
  meaningful YAML complexity for a use case the empty-commit path
  already covers.

## Open implementation details

To resolve during the implementation plan, not the spec:

- **Exact `claude[bot]` login string.** Confirmed during bootstrap from
  the first humanize run. The spec writes `claude[bot]`; correct in
  fix-forward if the live value differs.
- **Exact check-context string.** With `jobs.humanize.name: humanize`,
  the check API returns `humanize`. Some GitHub UIs render it as
  `Humanize Changelog / humanize`. The required_status_checks API uses
  the API value. Confirm at bootstrap.
- **`actor_id: 5` for admin bypass.** GitHub's repo-role IDs are
  poorly documented. The mapping I have is `1=Read, 2=Triage, 3=Write,
  4=Maintain, 5=Admin`, but I have not verified against this org. The
  spec captures intent; verify the literal during apply.
- **Whether `claude-code-action@v1` accepts `prompt_file:`.**
  `claude-code-review.yml` uses `prompt:`. If `prompt_file:` is
  unsupported, add a `Read prompt` step that does
  `prompt=$(cat .github/prompts/humanize-changelog.md)` and pass via
  `prompt:`. Trivial fix-up at implementation time.
- **Whether claude-code-action posts a default summary comment** for
  `prompt:`-style invocations or only for `@claude`-style. If silent,
  workflow logs are the only trace — acceptable.

## Cost

Per humanize run: ~5-15s wallclock, low single-digit-thousand tokens.
Under `CLAUDE_CODE_OAUTH_TOKEN` (Max), effectively free under quota.
A busy release week with 5 force-pushes/day = 5 humanize runs/day,
trivial quota usage.
