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
Push the commit to the PR branch — `git push` is enough; the action
has already configured the remote.

If the topmost section already looks humanized (e.g. release-plz did
not actually regenerate it on this trigger), still rewrite from scratch
from the commits in range. Output is deterministic from the commits,
not from the file's current contents.

## Update the PR description too

The release-plz PR body embeds the same cliff-generated section inside
a `<details><blockquote>` wrapper. Operators read that body, not the
file, when triaging the release PR — so it must match.

Steps (run via Bash):

1. Find the current PR number from the event:
   `PR_NUMBER="${GITHUB_REF##*/}"` is wrong on `pull_request`; use
   `gh pr view --json number --jq '.number'` from the checked-out
   branch — `gh` infers the PR from the branch.
2. Fetch the existing body:
   `gh pr view --json body --jq '.body' > /tmp/pr-body.md`.
3. Inside that body, find the topmost version block — the lines from
   `## [<VERSION>] - <DATE>` through (but not including) the closing
   `</blockquote>` of that release. Replace that block's body with the
   same humanized bullets you wrote into `CHANGELOG.md`. Keep the
   heading line and the surrounding `<details>` / `<blockquote>` HTML
   intact.
4. Write the result to `/tmp/pr-body.new.md` and apply:
   `gh pr edit --body-file /tmp/pr-body.new.md`.

If `gh pr edit` fails (e.g. token lacks pull-request write), surface
the error in the run log and continue — the committed `CHANGELOG.md`
is the source of truth and will be reflected in the next release.
