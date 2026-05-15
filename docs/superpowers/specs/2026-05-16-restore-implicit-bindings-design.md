# Restore Implicit Bindings Design

## Problem

`right agent init <target> --from-backup <backup>` restores `agent.yaml`,
`policy.yaml`, `data.db`, and `sandbox.tar.gz` into a new agent name. That is
valid for disaster recovery and cloning, but it currently treats omitted config
fields as if they belong to the target agent.

The concrete bug is Hindsight memory:

- `memory.provider: hindsight`
- `memory.bank_id` omitted
- source agent name: `right`
- restored target name: `right-drill`

At runtime, both the bot and the MCP aggregator resolve omitted
`memory.bank_id` to the current agent name. The restored clone therefore uses
bank `right-drill`, even though the backup was taken from bank `right`. Startup
then fails against Hindsight with `Bank 'right-drill' not found`.

The hidden problem is broader than this one API: restore must distinguish
between explicit copied state and clone-sensitive implicit defaults.

## Decision

Restore will make clone-sensitive implicit bindings explicit before the restored
agent can start.

For this spec, the only confirmed implicit external binding is Hindsight
`memory.bank_id`. Other name-derived values are either target-local runtime
names or explicit copied state.

When restoring an agent under the same source name, restore keeps existing
behavior and no new flag is required.

When restoring under a different target name and the backup has a Hindsight
memory config with omitted `memory.bank_id`, restore must choose one mode:

- `--preserve-source-bindings`: set `memory.bank_id` to the source agent's
  resolved bank ID.
- `--rebind-to-target`: leave `memory.bank_id` omitted, intentionally using the
  target agent name.
- `--memory-bank-id <id>`: set `memory.bank_id` to the supplied explicit value.

These options are mutually exclusive. Direct `--from-backup` restore is
non-interactive and must fail unless one is provided. Interactive restore
through the `right agent init <target>` wizard must ask the operator to choose.

`--preserve-source-bindings` is the recommended disaster-recovery mode.
`--rebind-to-target` is the explicit clone/fork mode.

## CLI

Extend restore flags on `right agent init`:

```text
right agent init <target> --from-backup <path> --preserve-source-bindings
right agent init <target> --from-backup <path> --rebind-to-target
right agent init <target> --from-backup <path> --memory-bank-id <id>
```

Rules:

- These flags are accepted only with `--from-backup`.
- The three restore binding options conflict with each other.
- `--memory-bank-id` requires a non-empty value.
- If the source agent and target agent are the same, the flags are optional.
- If the source agent is unknown and `memory.bank_id` was implicit, preserve
  mode cannot infer the source bank; restore must fail unless
  `--memory-bank-id` or `--rebind-to-target` is supplied.

The existing interactive wizard path that asks for a backup directory will use
the same resolver. If it detects an implicit clone-sensitive binding, it asks
the same choice instead of silently proceeding.

## Backup Manifest

Future full backups will include a non-secret manifest at the backup root:
`backup.json`.

```json
{
  "schema_version": 1,
  "source_agent": "right",
  "created_at": "2026-05-16T02:00:00Z",
  "sandbox_archive_root": "sandbox",
  "memory": {
    "provider": "hindsight",
    "bank_id_explicit": false,
    "resolved_bank_id": "right"
  },
  "explicit_state": {
    "has_telegram_token": true,
    "has_mcp_servers": true,
    "has_mcp_auth_tokens": true,
    "has_cron_specs": true
  }
}
```

The manifest must not contain secrets. It records enough resolved state to make
restore decisions deterministic on another machine.

`sandbox-only` backups do not contain `agent.yaml` today and cannot be restored
through `--from-backup`; they are out of scope for this restore flow.

## Legacy Backups

Existing backups do not have a manifest. Restore must support them when possible.

Legacy inference order:

1. If the backup path matches
   `$RIGHT_HOME/backups/<source-agent>/<timestamp>`, infer `source_agent` from
   the path.
2. If `agent.yaml` contains explicit `memory.bank_id`, no inference is needed.
3. If `agent.yaml` uses Hindsight and omits `memory.bank_id`, and the source
   cannot be inferred, restore cannot preserve source bindings without an
   explicit `--memory-bank-id`.

This preserves cross-machine safety: path-based inference is a convenience, not
a correctness requirement.

## Restore Mutation

Restore will copy config files first, then normalize restored `agent.yaml`
before sandbox creation/codegen can use it.

For Hindsight memory:

- Preserve mode writes `memory.bank_id: <source resolved bank>`.
- Explicit override writes `memory.bank_id: <provided value>`.
- Rebind mode leaves `memory.bank_id` omitted.
- Existing explicit `memory.bank_id` is copied as-is unless
  `--memory-bank-id` overrides it.

Both bot startup and aggregator startup should continue resolving
`memory.bank_id` the same way they do today. The restore fix belongs in the
restore path, not in duplicated runtime fallback logic.

## Explicit Copied State Warnings

Telegram tokens, MCP server rows, MCP auth tokens, allowlists, cron specs, and
agent secrets are explicit restored state. They should be copied for disaster
recovery.

When source and target names differ, restore should warn that the restored clone
may share external integrations with the source:

- same Telegram bot token can conflict if both agents run
- cron specs can duplicate scheduled delivery
- MCP auth tokens may point at the same third-party accounts

These warnings do not change restore binding mode. They are operator-awareness
checks, not hidden defaults.

## UAT

The acceptance test is a real restore drill:

1. Stop the source `right` bot so the Telegram token cannot conflict.
2. Restore the backup into `right-drill` with `--preserve-source-bindings`.
3. Start `right-drill`.
4. Confirm startup logs show Hindsight using the source bank and do not show
   `Bank 'right-drill' not found`.
5. Confirm Telegram bot response works.
6. Confirm MCP servers are registered and usable after restore.
7. Confirm the restored sandbox path is a new target sandbox, while durable
   files from the backup exist under `/sandbox`.

If UAT reveals another backup or restore correctness bug, stop implementation
and revise the spec or plan before layering additional fixes.

## Verification

Targeted tests should cover:

- manifest writing for full backups
- no manifest for unsupported `sandbox-only` restore path
- legacy source inference from backup path
- failure when source differs, Hindsight bank is implicit, and no restore mode
  is supplied in non-interactive restore
- preserve mode materializes `memory.bank_id`
- rebind mode leaves `memory.bank_id` omitted
- explicit override writes the provided bank ID
- explicit `memory.bank_id` remains unchanged unless overridden
- restore mode flags conflict correctly
- explicit copied state warnings are emitted for source/target clone restores

Final verification for implementation must run:

```text
devenv shell -- cargo test --workspace
```

## Out of Scope

- Automatically creating missing Hindsight banks.
- Rewriting Telegram tokens, MCP credentials, or cron specs during restore.
- Making `sandbox-only` backups restorable as full agents.
- Changing runtime Hindsight fallback semantics outside restore normalization.

## Open Questions

None.
