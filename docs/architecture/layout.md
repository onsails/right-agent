# Directory Layout & Logging

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## Runtime root

`~/.right/` is the runtime root (override with `--home`). Critical paths:

- `config.yaml` — global config (tunnel).
- `agents/<name>/` — per-agent state. Key files: `agent.yaml`,
  `allowlist.yaml`, `policy.yaml`, `data.db`,
  `.claude/.credentials.json` (symlink to `~/.claude/.credentials.json`,
  host-only — NOT uploaded to sandbox). Subdirs include `crons/`, `inbox/`,
  `outbox/`, and `tmp/` for staging during attachment transfer.
  Sandbox-internal: `/sandbox/.claude/projects/-sandbox/<sid>.jsonl` (CC
  project history, agent-readable for self-introspection via the
  `/right-reflect` skill); `/sandbox/.claude/logs/<sid>.log` (CC debug
  output, only present when `/debug` is on).
- `run/process-compose.yaml`, `run/state.json` (carries `pc_port` +
  `pc_api_token`), `run/internal.sock` (bot↔aggregator UDS),
  `run/ssh/<agent>.ssh-config`.
- `backups/<agent>/<YYYYMMDD-HHMM>/` — `sandbox.tar.gz` plus optional
  `agent.yaml` + `allowlist.yaml` + `data.db` + `policy.yaml` for full
  backups. `right agent backup` excludes rebuildable sandbox dirs by
  default (`.cache`, `.venv`, `.npm`, `.uv`); `--include-rebuildable` opts
  into forensic sandbox archives. No-sandbox archives, including destroy
  safety backups, exclude `data.db` and `data.db-*`; the only durable
  database backup file is the top-level `data.db`.
- `logs/<agent>.log.<date>` — per-agent daily log rotation.
  `mcp-aggregator.log` for the shared aggregator.
- `cache/whisper/ggml-<model>.bin` — STT models (downloaded at `right up`).

## Logging

Bot processes log to stderr + `~/.right/logs/<agent>.log` (daily rotation
via `tracing-appender`). Aggregator logs to stdout +
`~/.right/logs/mcp-aggregator.log`. See `docs/architecture/sessions.md`
for stream-logging detail.
