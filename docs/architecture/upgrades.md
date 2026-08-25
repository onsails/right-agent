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
4. Hot-reload machinery applies per category. Only one path remains —
   `BotRestart`: nothing extra, CC picks up the new file on next
   invocation. Egress is a typed value applied at sandbox create and the
   filesystem boundary is the microVM itself, so neither has a generated
   file to hot-apply or to drift-check.
5. Zero manual steps: every regenerated output reaches the agent on its
   next turn.

## Database ownership cutover

The Turso multiprocess-WAL release is a coordinated topology cutover, not a
rolling bot restart. Operators first stop the complete runtime and use
`right agent db-repair <name>...` for any legacy database: the explicit repair
preserves byte-identical database/WAL forensics, removes coordination sidecars
only from staging, validates a standalone snapshot, and swaps it atomically.
The runtime is not restarted automatically.

The new release starts the Aggregator and bots together. The Aggregator opens
standard-local `data.db`/`providers.db` owners before readiness; bots abort if
the typed readiness handshake fails and never fall back to a direct open. All
backup, restore, repair, destroy, rebootstrap, migration, and inspection paths
that open these databases directly require the shared runtime-quiescence gate.

## Identity mirror

For sandboxed agents, identity `AgentOwned` files are authoritative in
`/sandbox` once the sandbox exists. Host copies of `IDENTITY.md`, `SOUL.md`,
and `USER.md` are an explicit mirror, not the prompt source. Code that
needs a complete host mirror must call the identity mirror reconciliation
helper instead of assuming a prior user message ran reverse sync.

## No policy split

`policy.yaml` is retired. Network stance lives in `agent.yaml`
(`network_policy`) and becomes a `right_sandbox::Egress` value at create;
changing it needs a sandbox recreate, which is an explicit operator
action, not a codegen category.

## Non-goals

- Agent-owned content (`AgentOwned` files) — agent property; codegen never
  mutates them.
- Sandbox-backend upgrades — the microsandbox SDK pins its own runtime;
  see `Agent Sandbox Conventions` in `ARCHITECTURE.md`.
- SQLite-compatible schema — handled by `right-db` migrations (see `Local
  Database Rules` in `ARCHITECTURE.md`).
