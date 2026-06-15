# Humanize the latest CHANGELOG.md section

You are editing `CHANGELOG.md` in the **Right Agent** repo. Right Agent
is an opinionated, closed-box AI agent platform — operators run `right`
to spin up Telegram-driven Claude Code agents in OpenShell sandboxes.
The audience for this changelog is **those operators**, not contributors
browsing git history. They want to know what they will notice in this
release.

You are running because an admin commented `/humanize` on the release
PR. The checkout is the PR's head branch; you will edit, commit, and
push back to it.

## Why this is a map-reduce, not a skim

A single release can carry hundreds of commits (this repo routinely
ships 500-1000 between version tags). You **cannot** skim a thousand
commit subjects and reliably surface every operator-visible feature —
subtle but important work (a reworked subsystem landing as a pile of
`refactor`/`feat(db)` commits) gets dropped. So work in three phases:
**index → map → reduce**. Do not write a single bullet until you have
built the full feature index.

## Phase 0 — Determine the commit range

The commits in scope are those between the previous `v*` tag and HEAD:

    range="$(git describe --tags --abbrev=0 --match='v*' HEAD^)..HEAD"
    echo "$range"

Use `$range` in every command below.

## Phase 1 — Build the feature index (the spine)

This repo writes a design spec and an implementation plan per feature
under `docs/superpowers/{specs,plans}/`. **The set of spec/plan files
added in this range is a near-complete, pre-written table of contents of
the release.** Build the index from it:

    git diff --diff-filter=A --name-only "$range" -- \
      'docs/superpowers/specs/*' 'docs/superpowers/plans/*'

Each file name is `YYYY-MM-DD-<slug>(-design).md`. Collapse spec+plan
pairs that share a `<slug>` into one **feature cluster**. Strip the date
prefix and the `-design`/plan distinction to get the cluster name. This
gives you the canonical feature list — for example a `learning-*` /
`skill-*` group of clusters is a whole subsystem rebuild, even though no
single commit subject says "learning feature for operators".

**Catch features that shipped without a spec.** Some fixes/features land
with no design doc. After building the spec-based index, list candidate
clusters from the commits themselves and fold in any not already
covered:

    git log "$range" --no-merges --format='%h %s' \
      | grep -E '^[0-9a-f]+ (feat|fix)' 

Bucket these by scope/area. Any `feat`/`fix` bucket with operator-visible
behavior that no spec cluster already covers becomes its own cluster.

**Treat shipped agent-facing templates as behavior, not docs.** Built-in
skills and prompt/schema templates change what deployed agents do, even
when the commit type is `docs:` or `chore:`. After the commit scan, also
inspect changed files under these paths and add any operator-visible
behavior that no cluster already covers:

    git diff --name-only "$range" -- \
      'crates/right-codegen/skills/**' \
      'crates/right-codegen/templates/right/**' \
      'templates/right/**' \
      'crates/right-codegen/src/agent_def.rs' \
      'crates/right-codegen/src/mcp_instructions.rs'

Examples: a `right-cron` skill update that makes agents edit cron prompts
differently is behavior; a compiled prompt template that changes delivery
rules is behavior; a generated JSON schema change is behavior. Supporting
operator docs such as `PROMPT_SYSTEM.md` can explain the behavior but do
not create a changelog item by themselves.

Do not hide shipped agent-behavior changes inside an overloaded parent
cluster bullet. If a built-in skill or prompt template changes how an
agent handles a user command, it needs its own visible claim unless the
cluster has no other observable behavior. Example: "cron runs now name
linked skills" and "`right-cron` now slims old cron prompts toward those
skills when editing jobs" are two behaviors: runtime execution vs.
agent-mediated cron maintenance.

Your index is: **(spec/plan clusters) ∪ (orphan feat/fix clusters) ∪
(shipped agent-facing template behavior)**.

## Phase 2 — Map: one cluster at a time

Go through the index cluster by cluster. For each cluster:

1. Read its spec/plan (if any) for the *operator-facing intent* — that
   doc already explains why an operator cares. Read the file directly.
2. Find the cluster's commits and, when the title is ambiguous or the
   impact is unclear, `git show <sha>` to read the diff. Never infer a
   feature from a title alone.
3. Decide: does this change anything an operator can **observe**
   (behavior, a command, a Telegram/dashboard surface, a default, a
   limit, a cost)? If yes, write **one** bullet (occasionally two for a
   cluster that genuinely ships two unrelated user-visible things). If
   it is pure-internal, drop it and note why.

**Drop only truly internal clusters** — apply the internal/visible test
at the *cluster* level, not per commit. A user-visible feature whose
commits are mostly `refactor`/`feat(db)`/`test` is still user-visible;
keep it. Do not drop built-in skill or prompt-template changes as
"documentation-only" when they alter agent behavior. Things to drop:
test-only work, internal renames/moves, CI/lint fixups, schema-version
bumps with no behavior change, contributor-only docs, dev-only paths.

## Phase 3 — Reduce: assemble the section

1. Deduplicate bullets that describe the same observable change reached
   from different clusters. Agent-facing template changes are not
   duplicates of the runtime/storage feature they support when they change
   a different operator workflow.
2. Order from most to least impactful.
3. **Group by area when the release is large.** If you have more than
   ~8 bullets, put them under `### <Area>` headings (e.g. `### Learning &
   Skills`, `### Telegram & Agents`, `### Dashboard`, `### Memory`,
   `### Providers & Sandbox`, `### Platform`). Order areas by importance.
   For a small release (≤8 bullets) use a flat bullet list with no
   headings.
4. **There is no fixed bullet cap.** Ship one bullet per distinct
   operator-visible feature. A 1000-commit release legitimately produces
   15-30 bullets; a small one produces 3-7. Compressing a large release
   into a handful of bullets is the failure mode this prompt exists to
   prevent — do not do it.

If after dropping internal noise nothing operator-visible remains, write
one line instead:
`_Internal-only release. No operator-visible changes._`

## Voice

**Lead with user-visible consequence, not mechanism.**

| Don't                                                          | Do                                                                                                                     |
|----------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------|
| Drop JOIN to cron_specs in cron_runs delivery query            | Cron deliveries no longer get sent to the wrong chat when an agent's Telegram thread changes between schedule and run  |
| Restore ssh_exec cancel-safety via RAII pid guard              | Cancelling an in-flight agent command no longer leaves zombie ssh processes inside the sandbox                         |
| Extract shared post-turn learning pipeline module              | Agents now learn skills from recurring cron runs, not just live chats                                                  |
| Document right-cron prompt evolution                           | The built-in `right-cron` skill now checks linked skills before editing a cron and can simplify old step-by-step prompts into thinner goals that reuse those skills |

**Mark breaking changes** with a leading `**Breaking:**`. A commit is
breaking if it has `!` in the conventional-commit type (e.g. `feat!:`)
or a `BREAKING CHANGE:` trailer.

**Present tense, active voice.** "Cron retries failed deliveries." Not
"will now retry" or "are retried."

**Markdown.** Plain bullets; `### Area` headings only when grouping a
large release (Phase 3). No emojis, no nested bullets, no bold/italic
except `**Breaking:**` and the area headings.

## Phase 4 — Apply (changelog + PR body + coverage report)

The release-plz PR regenerated the topmost version section in
`CHANGELOG.md` (under `## [X.Y.Z] - YYYY-MM-DD`). **That is the only
section you rewrite.** Keep the heading line exactly as cliff produced
it (same version and date). Replace everything after the heading, up to
the next `## [` heading or end of file, with your bullets.

This run is comment-triggered and will not be cancelled by your own
push, so ordering is not fragile. Do all of:

1. Edit `CHANGELOG.md` in place (heading kept, body replaced).
2. Mirror the same bullets into the PR description. Fetch it with
   `gh pr view --json body --jq '.body' > "${RUNNER_TEMP:-/tmp}/pr-body.md"`,
   replace the body of the topmost `## [<VERSION>] - <DATE>` block (the
   lines from that heading up to, but not including, the closing
   `</blockquote>` of that release) with the same bullets, keep the
   surrounding `<details>`/`<blockquote>` HTML intact, do not touch
   earlier release blocks, and apply with
   `gh pr edit --body-file <file>`.
3. Commit and push to the PR branch with this message exactly:

       chore(changelog): humanize v<VERSION>

   where `<VERSION>` is the version from the section heading.
4. Post a short coverage report as a PR comment so the admin can audit
   for misses and re-run `/humanize` if needed:
   `gh pr comment --body "<report>"`. The report lists every cluster you
   found and, for each, whether it was **included** or **dropped (reason)**.
   This is the safety net against silent omissions — be honest about what
   you left out.

If `gh pr edit` or `gh pr comment` fails (e.g. token lacks write),
continue anyway — the committed `CHANGELOG.md` is the source of truth.

## Accuracy

Every claim in a bullet must be supported by the commits in range. If a
commit's real impact is smaller or different than its title implies,
write the bullet from the diff, not the title. If you cannot find
evidence for a claim, drop it. Ship accurate bullets only — a missing
feature can be added on re-run; a fabricated one misleads operators.
