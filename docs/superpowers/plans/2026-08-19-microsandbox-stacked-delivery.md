# microsandbox migration — stacked delivery plan

Execution plan for GitHub issue
[onsails/right-agent#172](https://github.com/onsails/right-agent/issues/172).
Design rationale lives in
`docs/superpowers/specs/2026-08-19-microsandbox-migration-design.md` and
`docs/adr/0001..0003`. This document defines only **how the work is cut,
reviewed, and landed** — not what it does.

## Why stacked

The migration is one logical change that touches ~20 crates. A single PR is
unreviewable; six independent PRs do not compile in isolation. So the work
lands as a **stack**: each stage branches from the previous stage, and each
stage PR targets its parent branch. A reviewer of stage *N* sees only stage
*N*'s diff. One extra PR from the top of the stack to `master` carries the
whole migration and is the only PR that merges.

## Branch stack

| Stage | Branch | PR base | Scope |
| --- | --- | --- | --- |
| 1 | `msb/01-assumptions` | `master` | Live-microVM verification of the 7 load-bearing assumptions; pinned SDK dependency; verdict document |
| 2 | `msb/02-sandbox-crate` | `msb/01-assumptions` | `right-sandbox` crate: lifecycle, streaming exec, guest filesystem, egress translation, secret bindings |
| 3 | `msb/03-provider-store` | `msb/02-sandbox-crate` | Right-owned provider store; internal API and dashboard repointed; built-in catalog as constants |
| 4 | `msb/04-rewire` | `msb/03-provider-store` | Bot/codegen/CLI rewiring; sandboxless mode removed; MCP aggregator binds loopback |
| 5 | `msb/05-migrate-command` | `msb/04-rewire` | `right agent migrate-sandbox`; manifest selection; bot refuses unmigrated agents |
| 6 | `msb/06-drop-openshell` | `msb/05-migrate-command` | Delete `right-openshell`, vendored proto, proto-compat CI job, stale docs |
| — | `msb/06-drop-openshell` | `master` | **Final PR.** The whole migration. The only PR that merges. |

Rules:

- A stage branch is created from its parent's tip **when the stage starts**,
  never earlier. No stage is rebased once its PR is open unless its parent
  moved.
- If review of stage *N* demands a change, the fix lands on stage *N*'s
  branch and every later branch is rebased onto it. Fixes never land on a
  later stage "because it is already open".
- Stage PRs are review vehicles. They are never merged individually.
- Every stage branch must compile: `cargo check --workspace` is the floor
  for opening a stage PR. Stages that cannot compile alone are a signal the
  cut is wrong.

## Worktrees

One worktree per stage under the project root, per repo convention:

```
.worktrees/msb-01-assumptions
.worktrees/msb-02-sandbox-crate
...
.worktrees/msb-06-drop-openshell
```

A worktree is created when its stage starts and removed after its stage PR
is approved and the next stage has branched from it. Implementation
subagents are pinned to exactly one worktree path and must never edit the
main checkout.

## Review protocol

Each stage gets a dedicated review pass before the next stage branches:

1. Implementation subagents finish the stage and run **targeted** tests only
   (`devenv shell -- cargo nextest run -p <crate> <filter>`). They do not run
   the full workspace suite, do not run formatters, and do not run linters.
2. The orchestrator runs `cargo check --workspace` in the stage worktree.
3. A `review-rust-code` subagent reviews **only** `git diff <parent-branch>..HEAD`
   inside that worktree. It is told the parent branch explicitly and told not
   to comment on code outside that range. It reports findings and never fixes.
4. Findings become TODOs. Blocking findings are fixed on the stage branch
   before the next stage branches.

## Verification cadence

- Per stage: targeted package tests plus `cargo check --workspace`.
- After stage 6 only: `devenv shell -- cargo nextest run --workspace` and
  `devenv shell -- cargo test --doc --workspace`. Mandatory, from inside the
  stage-6 worktree.
- Live-microVM tests carry the `ci_msb_` name prefix and `#[ignore]` with a
  `ci-msb: ...` reason. They cannot run on GitHub-hosted runners; the gap is
  accepted and recorded in the architecture docs.

## Pilot and rollback

This host is the pilot. Before any implementation work touches the host's
agent state, `~/.right/agents`, `~/.right/config.yaml`,
`~/.right/cloudflared-config.yml` and `~/.right/backups` are archived to
`~/backups/rightclaw/right-home-pre-microsandbox-<timestamp>.tar.gz`.

Rollback for the pilot is: restore that archive, check out `master`, and
`right up`. The OpenShell sandboxes are not deleted by any stage before
stage 5's migration command runs, and that command deletes an old sandbox
only after a verified restore.

## Assumption gate

Stage 1 is a gate, not a formality. Each of the seven assumptions in issue
#172 gets a recorded verdict of **verified**, **contradicted**, or
**deferred with named risk**. A contradicted assumption stops the stack and
returns to design; it does not get worked around in stage 2.
