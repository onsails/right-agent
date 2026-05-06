# Humanize release-plz Changelog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the cliff-generated section of every release-plz PR with operator-facing prose, generated automatically by `anthropics/claude-code-action@v1` and gated by a required check on master.

**Architecture:** Three new files (`.github/workflows/humanize-changelog.yml`, `.github/prompts/humanize-changelog.md`, `.github/rulesets/master.json`) ship in one PR. After that PR merges, wait one release cycle to confirm the live `sender.login` and check-context strings, then apply the ruleset via `gh api`. Bootstrap is intentionally split across two phases to avoid a "wrong loop guard + blocking ruleset" two-bug state.

**Tech Stack:** GitHub Actions YAML, GitHub Repository Rulesets, `anthropics/claude-code-action@v1`, existing `CLAUDE_CODE_OAUTH_TOKEN` secret, `actionlint`, `jq`, `gh`.

**Spec:** `docs/superpowers/specs/2026-05-06-humanize-changelog-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `.github/prompts/humanize-changelog.md` | Create | The humanize prompt: scope, output shape, voice rules with before/after examples, drop list, commit message format. |
| `.github/workflows/humanize-changelog.yml` | Create | Triggers on release-plz PR events; filters to release-plz-authored "chore: release" PRs; loop-guards out Claude's own commits; calls `claude-code-action@v1`. |
| `.github/rulesets/master.json` | Create | Repository Ruleset definition. Required for change-tracking and reproducibility; not auto-applied (one-time `gh api` call deferred to Task 6). |

The implementation has six tasks. Tasks 1-4 are committed and shipped in one PR. Tasks 5-6 are post-merge bootstrap steps gated by live telemetry from the next release-plz PR.

---

## Task 1: Add the humanize prompt file

**Files:**
- Create: `.github/prompts/humanize-changelog.md`

The prompt is the heart of this feature — it determines whether the rewrite is good. We add it first so the workflow YAML can reference it.

- [ ] **Step 1: Create the prompt file with the full content from the spec**

Create `.github/prompts/humanize-changelog.md` with this exact content:

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

- [ ] **Step 2: Verify file was written and content matches**

Run: `wc -l .github/prompts/humanize-changelog.md`
Expected: ~75 lines (the exact number depends on trailing newlines; should be 70-80).

Run: `head -3 .github/prompts/humanize-changelog.md`
Expected output (exact match for the top three lines):

```
# Humanize the latest CHANGELOG.md section

You are editing `CHANGELOG.md` in the **Right Agent** repo. Right Agent
```

- [ ] **Step 3: Commit**

```bash
git add .github/prompts/humanize-changelog.md
git commit -m "feat(ci): add humanize-changelog prompt for claude-code-action"
```

---

## Task 2: Add the humanize workflow

**Files:**
- Create: `.github/workflows/humanize-changelog.yml`

Workflow that fires on release-plz PR events, filters to release PRs only, loop-guards out Claude's own commits, and invokes `claude-code-action@v1` with the prompt file from Task 1.

- [ ] **Step 1: Create the workflow file with this exact content**

Create `.github/workflows/humanize-changelog.yml`:

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

- [ ] **Step 2: Lint with actionlint**

`actionlint` is the standard linter for GitHub Actions YAML. It validates schema, expression syntax, and shell scripts inside `run:` blocks.

Run: `nix run nixpkgs#actionlint -- .github/workflows/humanize-changelog.yml`
Expected: silent exit 0 (no findings).

If actionlint reports `unknown input "prompt_file" for action ...`, this means the action's metadata doesn't expose `prompt_file` as a documented input. **Do not fail at this step** — `actionlint` only checks against actions it can resolve. The `claude-code-review.yml` workflow uses `prompt:` rather than `prompt_file:`. If the action genuinely does not support `prompt_file:`, fix-forward as described in Step 3 below.

- [ ] **Step 3: Verify `prompt_file:` is supported by claude-code-action@v1**

Run: `gh api repos/anthropics/claude-code-action/contents/action.yml --jq '.content' | base64 -d | rg -i 'prompt_file|prompt:'`
Expected: output contains both `prompt:` and `prompt_file:` definitions.

If `prompt_file:` is **not** present in the action's `inputs:`, replace the workflow's last step with:

```yaml
      - name: Read prompt
        id: prompt
        run: |
          {
            echo 'value<<PROMPT_EOF'
            cat .github/prompts/humanize-changelog.md
            echo 'PROMPT_EOF'
          } >> "$GITHUB_OUTPUT"

      - name: Run Claude humanize
        uses: anthropics/claude-code-action@v1
        with:
          claude_code_oauth_token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}
          prompt: ${{ steps.prompt.outputs.value }}
```

Then re-run actionlint:

Run: `nix run nixpkgs#actionlint -- .github/workflows/humanize-changelog.yml`
Expected: silent exit 0.

- [ ] **Step 4: Sanity-check the filter expression by hand**

Read the `if:` block out loud as three boolean clauses:
1. `startsWith(github.event.pull_request.title, 'chore: release')` — release-plz PRs only.
2. `github.event.pull_request.user.login == 'github-actions[bot]'` — author is the bot.
3. `github.event.sender.login != 'claude[bot]'` — loop guard.

Verify that `&&` joins them at top level (not `||`). Verify the multi-line YAML uses the `|` literal block scalar so that newlines are preserved as whitespace inside the GitHub Actions expression.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/humanize-changelog.yml
git commit -m "feat(ci): add humanize-changelog workflow"
```

---

## Task 3: Add the master ruleset JSON

**Files:**
- Create: `.github/rulesets/master.json`

The ruleset captures the desired branch protection state in code. It is **not** automatically applied — the apply step is deferred to Task 6 after live telemetry confirms the loop-guard string and check-context name.

- [ ] **Step 1: Verify the rulesets directory does not exist**

Run: `ls .github/rulesets/ 2>&1 || echo "missing - good"`
Expected: either an empty directory listing or `missing - good`. If the directory exists with files, stop and inspect — it shouldn't.

- [ ] **Step 2: Create the directory and the ruleset JSON**

```bash
mkdir -p .github/rulesets
```

Create `.github/rulesets/master.json` with this exact content:

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

- [ ] **Step 3: Validate JSON syntax**

Run: `jq empty < .github/rulesets/master.json`
Expected: silent exit 0. Any output is a parse error — fix and re-run.

- [ ] **Step 4: Validate ruleset semantics by re-reading via jq**

Run: `jq '{name, enforcement, rules: [.rules[].type], required_checks: .rules[] | select(.type=="required_status_checks") | .parameters.required_status_checks}' < .github/rulesets/master.json`

Expected output:

```json
{
  "name": "master-required-checks",
  "enforcement": "active",
  "rules": [
    "pull_request",
    "required_status_checks"
  ],
  "required_checks": [
    {
      "context": "humanize"
    }
  ]
}
```

- [ ] **Step 5: Commit**

```bash
git add .github/rulesets/master.json
git commit -m "feat(ci): add master branch ruleset for humanize-changelog gating"
```

---

## Task 4: Open the bootstrap PR

**Files (none created/modified in this task — branch + PR):**

This task ships Tasks 1-3 in a single PR for review and merge. The ruleset apply (Task 6) is deliberately a separate manual step after merge.

- [ ] **Step 1: Confirm we are on a feature branch, not master**

Run: `git rev-parse --abbrev-ref HEAD`
Expected: a branch name that is **not** `master`. If output is `master`, create a feature branch:

```bash
git checkout -b ci/humanize-changelog
```

(If the brainstorming or writing-plans skill already moved us into a worktree on a feature branch, this is a no-op confirmation.)

- [ ] **Step 2: Confirm the three commits from Tasks 1-3 are on this branch**

Run: `git log master..HEAD --oneline`
Expected: three commits in this order (oldest first):

```
<sha>  feat(ci): add humanize-changelog prompt for claude-code-action
<sha>  feat(ci): add humanize-changelog workflow
<sha>  feat(ci): add master branch ruleset for humanize-changelog gating
```

If you see fewer or more commits, stop and inspect.

- [ ] **Step 3: Push the branch**

```bash
git push -u origin HEAD
```

- [ ] **Step 4: Open the PR**

```bash
gh pr create \
  --title "ci: humanize release-plz changelog with claude-code-action" \
  --body "$(cat <<'EOF'
Implements `docs/superpowers/specs/2026-05-06-humanize-changelog-design.md`.

## Summary

- Adds `.github/prompts/humanize-changelog.md` — operator-facing rewrite rules with before/after examples and a drop list.
- Adds `.github/workflows/humanize-changelog.yml` — fires on release-plz PR events, filters to release PRs, loop-guards Claude's own commits, calls `anthropics/claude-code-action@v1`.
- Adds `.github/rulesets/master.json` — defines the required-checks ruleset. **Not auto-applied** — applied separately via `gh api` after one live release-plz cycle confirms the loop-guard string and check-context name (see spec § Bootstrap order).

## Test plan

- [ ] Workflow file passes `actionlint`.
- [ ] Ruleset JSON parses with `jq`.
- [ ] After merge: wait for next release-plz PR, observe humanize run, confirm `sender.login` and check-run name match the spec.
- [ ] After confirmation: apply the ruleset via `gh api`.
EOF
)"
```

- [ ] **Step 5: Watch CI on the PR**

Run: `gh pr checks --watch`
Expected: existing checks (`Release-plz PR`, `Claude Code Review`) run and pass. Note that the new `humanize` check **does not run on this PR** because the PR title doesn't start with `chore: release` and the author isn't `github-actions[bot]` — that's the filter doing its job.

- [ ] **Step 6: Merge once approved**

After review:

```bash
gh pr merge --squash --auto
```

(Or wait for manual merge. The auto flag merges as soon as required checks pass and review approves.)

---

## Task 5: Bootstrap-cycle verification

**Files (none modified in this task — telemetry only):**

After merge, wait for the next release-plz PR. This task captures the loop-guard string and check-context name from a real run, before we apply the ruleset.

This is a manual verification task — there is no test to run before observation. Do not skip; the spec calls out this as the cheap insurance against a two-bug state.

- [ ] **Step 1: Wait for the next release-plz PR to open or be force-pushed**

Run: `gh pr list --state open --search 'chore: release'`
Expected: one open PR titled `chore: release v0.X.Y` once release-plz has run.

If no such PR exists yet, wait for the next push to master that includes a non-release conventional commit. Then re-run the command.

- [ ] **Step 2: Observe the humanize workflow run**

```bash
PR_NUMBER=$(gh pr list --state open --search 'chore: release' --json number --jq '.[0].number')
gh run list --workflow="Humanize Changelog" --branch "$(gh pr view "$PR_NUMBER" --json headRefName --jq '.headRefName')" --limit 1
```

Expected: one run, status `completed`, conclusion `success`. If it failed, stop and read the failure log:

```bash
RUN_ID=$(gh run list --workflow="Humanize Changelog" --limit 1 --json databaseId --jq '.[0].databaseId')
gh run view "$RUN_ID" --log-failed
```

The most likely cause of first-run failure is the `prompt_file:` vs `prompt:` action input — if the failure log says "Unexpected input(s) 'prompt_file'", apply the fix-forward described in Task 2 Step 3.

- [ ] **Step 3: Confirm the loop-guard string by reading the second-run skip**

After the humanize run pushes its commit to the PR, GitHub fires another `synchronize` event. The job filter should skip this second invocation.

```bash
gh run list --workflow="Humanize Changelog" --limit 5
```

Expected: at least two runs. The second-most-recent should have `conclusion = success` and `status = completed` very quickly (skipped at the job level — no work done).

To confirm the skip happened on the loop guard (not on a different filter clause), inspect the event payload of the second run:

```bash
RUN_ID=$(gh run list --workflow="Humanize Changelog" --limit 5 --json databaseId --jq '.[1].databaseId')
gh run view "$RUN_ID" --json event --jq '.event'
```

Expected: `pull_request`. Then read the head commit's author from the PR:

```bash
gh pr view "$PR_NUMBER" --json commits --jq '.commits[-1].authors[0].login'
```

Expected: `claude[bot]` (or whatever string. **Record this string.** It must match the workflow's `if:` clause — `github.event.sender.login != 'claude[bot]'`.

- [ ] **Step 4: Confirm the check-context name on the PR**

```bash
HEAD_SHA=$(gh pr view "$PR_NUMBER" --json headRefOid --jq '.headRefOid')
gh api "repos/onsails/right-agent/commits/$HEAD_SHA/check-runs" --jq '.check_runs[].name'
```

Expected: a list of check names. One of them must be `humanize` (matching the ruleset's `context` value). **Record this string.**

- [ ] **Step 5: If either string disagrees with the spec, fix-forward**

If the recorded loop-guard login is not `claude[bot]`, edit `.github/workflows/humanize-changelog.yml` and update the `if:` clause:

```yaml
github.event.sender.login != '<recorded-string>'
```

If the recorded check-name is not `humanize`, edit `.github/rulesets/master.json` and update the `context` value.

Commit and push the fix:

```bash
git add .github/workflows/humanize-changelog.yml .github/rulesets/master.json
git commit -m "fix(ci): correct loop-guard string and/or check-context name"
git push
```

Then return to Step 1 of this task and verify on the next release-plz cycle.

If both strings match the spec, proceed to Task 6.

---

## Task 6: Apply the ruleset

**Files (none modified — applies committed JSON to GitHub):**

Once Task 5 confirms both strings, apply the ruleset to enforce the gate.

- [ ] **Step 1: Apply the ruleset via gh api**

```bash
gh api -X POST /repos/onsails/right-agent/rulesets \
  -H "Accept: application/vnd.github+json" \
  --input .github/rulesets/master.json
```

Expected: HTTP 201 response with a JSON body containing `id` (an integer), `name: "master-required-checks"`, `enforcement: "active"`. Save the `id`:

```bash
RULESET_ID=$(gh api -X POST /repos/onsails/right-agent/rulesets \
  -H "Accept: application/vnd.github+json" \
  --input .github/rulesets/master.json --jq '.id')
echo "Ruleset ID: $RULESET_ID"
```

If you already ran the create call once and it succeeded, skip the re-create and fetch the existing ID:

```bash
gh api /repos/onsails/right-agent/rulesets --jq '.[] | select(.name=="master-required-checks") | .id'
```

- [ ] **Step 2: Verify the ruleset is active and gates master**

```bash
gh api /repos/onsails/right-agent/rulesets/$RULESET_ID --jq '{name, enforcement, target, rules: [.rules[].type]}'
```

Expected output:

```json
{
  "name": "master-required-checks",
  "enforcement": "active",
  "target": "branch",
  "rules": [
    "pull_request",
    "required_status_checks"
  ]
}
```

- [ ] **Step 3: Confirm `actor_id: 5` resolved to "Admin"**

GitHub's repo-role IDs are not officially documented as numeric constants. Verify the bypass is for the admin role, not some other role:

```bash
gh api /repos/onsails/right-agent/rulesets/$RULESET_ID --jq '.bypass_actors'
```

Expected: a single entry with `actor_type: "RepositoryRole"` and `actor_id: 5`. If GitHub's UI (Settings → Rules → master-required-checks) shows the bypass actor as something other than "Admin" / "Repository admin", `actor_id` is wrong:

1. Read the correct ID from the UI's URL or by inspecting an existing ruleset on another repo where the admin bypass is known to work.
2. Edit `.github/rulesets/master.json` and update `actor_id`.
3. Update the live ruleset:

```bash
gh api -X PUT /repos/onsails/right-agent/rulesets/$RULESET_ID \
  -H "Accept: application/vnd.github+json" \
  --input .github/rulesets/master.json
```

Then commit the JSON correction:

```bash
git add .github/rulesets/master.json
git commit -m "fix(ci): correct admin bypass actor_id in master ruleset"
git push
```

- [ ] **Step 4: End-to-end verify the gate by visiting an open release-plz PR**

```bash
PR_NUMBER=$(gh pr list --state open --search 'chore: release' --json number --jq '.[0].number')
gh pr view "$PR_NUMBER" --json statusCheckRollup --jq '.statusCheckRollup[] | select(.name=="humanize") | {name, state}'
```

Expected: an entry like `{"name": "humanize", "state": "SUCCESS"}` if the latest humanize run succeeded. The PR's "Merge" button in the UI should show "Required" next to the humanize check.

If no release-plz PR is currently open, force a release-plz cycle to verify the gate end-to-end:

```bash
git commit --allow-empty -m "chore: trigger release-plz to verify humanize gate"
git push origin master
```

(Only do this if you have permission to push directly to master, which the ruleset's `bypass_mode: always` permits for admins. Otherwise, open a tiny no-op PR through the normal flow.)

The next release-plz PR should show:
- `humanize` check pending (and required).
- `humanize` check resolves to success once Claude finishes.
- "Merge" button enabled only after success.

- [ ] **Step 5: Document the live ruleset ID in the spec**

Edit `docs/superpowers/specs/2026-05-06-humanize-changelog-design.md` and add a short note under "The ruleset" with the recorded `RULESET_ID`, so future edits know which ID to `PUT`.

```bash
git add docs/superpowers/specs/2026-05-06-humanize-changelog-design.md
git commit -m "docs(spec): record live ruleset id for humanize gate"
git push
```

---

## Self-review checklist

After implementing all tasks, run through:

- [ ] All three new files exist on master: `.github/prompts/humanize-changelog.md`, `.github/workflows/humanize-changelog.yml`, `.github/rulesets/master.json`.
- [ ] At least one release-plz PR has been humanized end-to-end (cliff section replaced with operator-facing bullets).
- [ ] The loop-guard string and check-context name match between workflow YAML, ruleset JSON, and live telemetry.
- [ ] The ruleset is `enforcement: "active"` and shows up in repo Settings → Rules.
- [ ] Admin bypass works (try merging a release-plz PR with a deliberately failing humanize check; admin should be able to override).
- [ ] No `workflow_dispatch:` was added (out of scope per spec).
- [ ] No PR-time `cargo check` was added (out of scope per spec).

## Operational reference

After this plan ships, day-to-day operations are:

- **Re-run humanize on an existing release PR:** push an empty commit to master.
  ```bash
  git commit --allow-empty -m "chore: trigger release-plz" && git push origin master
  ```
- **Override a failing humanize gate (admin only):** merge via the GitHub UI; `bypass_mode: always` permits it.
- **Disable humanize temporarily:** change `enforcement: "active"` to `"disabled"` in `.github/rulesets/master.json`, `gh api -X PUT` it, and revert when done.
