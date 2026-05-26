# Upgrade Flow

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

Load-bearing rules (codegen categories, helper API, rules for adding a new
codegen output) stay in `ARCHITECTURE.md`. This file describes what
happens during a typical upgrade.

## Walkthrough

1. Code change merged.
2. User runs `right restart <agent>` (or the bot restarts naturally via
   process-compose `on_failure`).
3. `run_single_agent_codegen` rewrites every `Regenerated` file.
4. Hot-reload machinery applies per category:
   - `BotRestart`: nothing extra — CC picks up the new file on next
     invocation.
   - `SandboxPolicyApply`: `write_and_apply_sandbox_policy` hot-reloads via
     `openshell policy set --wait`.
   - `SandboxRecreate`: bot startup compares active vs on-disk policy via
     `filesystem_policy_changed`. On drift, logs a WARN telling the operator
     to run `right agent config <agent>`, which invokes
     `maybe_migrate_sandbox`. No automatic migration — it's disruptive and
     requires operator consent.
5. For `BotRestart` / `SandboxPolicyApply`: zero manual steps.
6. For `SandboxRecreate`: one follow-up command from the operator.

## Identity mirror

For sandboxed agents, identity `AgentOwned` files are authoritative in
`/sandbox` once the sandbox exists. Host copies of `IDENTITY.md`, `SOUL.md`,
and `USER.md` are an explicit mirror, not the prompt source. Code that
needs a complete host mirror must call the identity mirror reconciliation
helper instead of assuming a prior user message ran reverse sync.

## Policy split

`policy.yaml` mixes a hot-reloadable network section and a recreate-only
filesystem section. It's registered as the stricter
`Regenerated(SandboxRecreate)`; runtime discriminates via
`openshell::filesystem_policy_changed`.

## Non-goals

- Agent-owned content (`AgentOwned` files) — agent property; codegen never
  mutates them.
- OpenShell server upgrades — covered by `OpenShell Integration
  Conventions`.
- SQLite-compatible schema — handled by `right-db` migrations (see `Local
  Database Rules` in `ARCHITECTURE.md`).
