# Backup Rebuildable Excludes Design

## Problem

`right agent backup <name>` currently archives all of `/sandbox` for sandboxed
agents. That produces a forensic snapshot, but it also captures rebuildable
agent-writable dependency state. Empirical checks on `him` showed `/sandbox` is
9.1G, mostly:

- `/sandbox/.venv` - 5.0G
- `/sandbox/.cache` - 2.9G
- `/sandbox/.local` - 885M

The largest directories are writable by the sandbox user, so they are not base
image state. They are still usually rebuildable and should not dominate the
default disaster-recovery backup.

## Decision

Default backup remains broad: archive everything under `/sandbox` except an
explicit rebuildable exclude set.

Default excluded paths:

- `sandbox/.cache`
- `sandbox/.venv`
- `sandbox/.npm`
- `sandbox/.uv`

Default included paths therefore include top-level files and any unknown
user-created directories, plus:

- `sandbox/.agents`
- `sandbox/.claude`
- `sandbox/.config`
- `sandbox/.local`
- `sandbox/.platform`
- `sandbox/crons`
- `sandbox/inbox`
- `sandbox/outbox`

Including `.platform` by default is deliberate. It is empirically non-writable,
but including it avoids surprising restore gaps and keeps the backup shape easy
to inspect. Codegen may still overwrite platform-owned files after restore.

## CLI

Keep existing behavior for host/config artifacts:

```text
right agent backup <name>
right agent backup <name> --sandbox-only
```

Change sandbox archive behavior:

- `right agent backup <name>` archives `/sandbox` with the default rebuildable
  excludes above.
- `right agent backup <name> --sandbox-only` uses the same default rebuildable
  excludes, but still skips host `agent.yaml`, `policy.yaml`, and `data.db`.
- `right agent backup <name> --include-rebuildable` archives all of `/sandbox`,
  including `.cache`, `.venv`, `.npm`, and `.uv`.
- `right agent backup <name> --sandbox-only --include-rebuildable` produces the
  current forensic sandbox-only behavior.

The new flag name is explicit because it says what changes: rebuildable bulk is
included. Avoid names like `--full`; "full backup" already means sandbox plus
host config/database in this CLI.

## Archive Shape

All sandbox tarballs keep `sandbox/...` paths. The default backup must not
switch to an include-list model because that would miss unknown user-created
directories. It must instead pass tar excludes while archiving `/sandbox`.

For OpenShell sandboxes, use remote GNU tar excludes equivalent to:

```text
--exclude=./.cache
--exclude=./.cache/*
--exclude=./.venv
--exclude=./.venv/*
--exclude=./.npm
--exclude=./.npm/*
--exclude=./.uv
--exclude=./.uv/*
```

The implementation may use the existing transform-based command that reads from
`/sandbox` and writes archive paths under `sandbox/`. GNU tar evaluates
`--exclude` before `--transform`, so excludes must match the pre-transform
names (`./.cache`), while verification checks the transformed archive names
(`sandbox/.cache`).

For no-sandbox agents, keep the existing `data.db` exclusion because `data.db`
is copied via SQLite `VACUUM INTO`. Add equivalent exclusions for agent-dir
children `.cache`, `.venv`, `.npm`, and `.uv` unless `--include-rebuildable` is
passed.

## Verification

Default backup verification should prove:

- `sandbox.tar.gz` exists and is non-empty.
- `tar -tzf sandbox.tar.gz` exits 0.
- The tar list contains `sandbox/`.
- The tar list does not contain the default excluded directories when
  `--include-rebuildable` is absent.
- Host artifacts are present for non-`--sandbox-only` backups:
  `agent.yaml`, `data.db`, and `policy.yaml` when present in the source agent.
- `data.db` passes `PRAGMA integrity_check`.

`--include-rebuildable` verification should prove:

- `tar -tzf sandbox.tar.gz` exits 0.
- The tar list contains `sandbox/`.
- If excluded directories exist in the source sandbox, their archive paths are
  present.

## Restore Semantics

Restore remains tar extraction into `/`. A default backup intentionally omits
rebuildable state; after restore the agent may need to reinstall Python/npm/uv
dependencies or repopulate caches. This is acceptable because default backup is
for durable recovery, not bit-for-bit forensics.

For exact forensic recovery, users must opt in with `--include-rebuildable`.

## Error Handling

Backup must fail if tar exits non-zero. The error should include remote tar
stderr so live-file warnings and permission failures are observable. Do not
silently accept partial archives.

## Open Questions

None. `.local` stays included by default even though Claude versions can be
large, because `.local/bin` can contain user-installed tools and there is not
yet a reliable boundary between rebuildable runtime and user tools under
`.local`.
