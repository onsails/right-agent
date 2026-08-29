# Directory Layout & Logging

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Runtime root

`~/.right/` is the runtime root (override with `--home`). Critical paths:

- `config.yaml` — global config (tunnel).
- `agents/<name>/` — per-agent state. Key files: `agent.yaml`,
  `allowlist.yaml`, and `data.db`. During runtime the Aggregator
  owns the sole standard-local writable connection; `data.db-wal` and
  `data.db-shm` may exist, but legacy `data.db-tshm` must not be created.
  `.claude/.credentials.json` is a host-only symlink to
  `~/.claude/.credentials.json` and is not uploaded to the sandbox. Subdirs
  include `crons/`, `inbox/`, `outbox/`, and attachment-staging `tmp/`.
  Sandbox-internal: `/sandbox/.claude/projects/-sandbox/<sid>.jsonl` (CC
  project history, agent-readable for self-introspection via the
  `/right-reflect` skill); `/sandbox/.claude/logs/<sid>.log` (CC debug
  output, only present when `/debug` is on).
- `providers.db` — provider authority and encrypted-at-rest-by-host-permissions credential store; the Aggregator owns its only live connection.
- `run/process-compose.yaml`, `run/state.json` (carries `pc_port` +
  `pc_api_token`), and `run/internal.sock` (mode 0600 typed bot↔Aggregator DB
  and control-plane IPC).
- `backups/<agent>/<YYYYMMDD-HHMM>/` — full `right agent backup` output:
  `sandbox.tar.gz`, `backup.json`, and optional `agent.yaml`, `allowlist.yaml`,
  and `data.db`. Pre-destroy backups omit `backup.json` and may also preserve a
  legacy `policy.yaml` when an upgraded agent still has that retired file.
  `right agent backup` excludes rebuildable sandbox dirs by default (`.cache`,
  `.venv`, `.npm`, `.uv`); `--include-rebuildable` opts into forensic sandbox
  archives. Normal agent backups contain only the top-level `data.db` snapshot,
  never its sidecars; database-repair forensic backups preserve a different set.
- `logs/<agent>.log.<date>` — per-agent daily log rotation.
  `mcp-aggregator.log` for the shared aggregator.
- `cache/whisper/ggml-<model>.bin` — STT models (downloaded at `right up`).
- `cache/claude-code/` — mode-0700 host cache for pinned Claude Code artifacts.
  A bounded exclusive lock serializes verification/download; final artifacts
  are content-addressed and mode 0555. Corrupt regular entries are removed and
  downloaded again, while symlink or non-regular cache entries fail closed.

Inside each sandbox, the authoritative fallback runtime is outside the
guest-owned `/sandbox` tree: root-owned `/opt/right/claude/` stores verified
versions and `/opt/right/bin/claude` is the atomically replaced active symlink.
Activation retains the current version and at most one prior target. The
guest-owned `/sandbox/.local/bin/claude` intentionally precedes this fallback in
PATH so the bot-managed upgrade command can override it.

## Logging

Bot processes log to stderr + `~/.right/logs/<agent>.log` (daily rotation
via `tracing-appender`). Aggregator logs to stdout +
`~/.right/logs/mcp-aggregator.log`. See `docs/architecture/sessions.md`
for stream-logging detail.
