# Phase 37: fix-v3-2-uat-gaps-tunnel-setup-flow-tunnel-hostname-dns-rout - Context

**Gathered:** 2026-04-04
**Status:** Ready for planning

<domain>
## Phase Boundary

Fix all 5 UAT gaps identified in `.planning/phases/v3.2-mcp-oauth-uat/v3.2-UAT.md`:
1. Tunnel setup flow — --tunnel-hostname arg, DNS routing wrapper, doctor checks (MAJOR)
2. MCP tracing logs in bot handlers (minor)
3. rightclaw up MCP warning visibility before TUI launches (minor)
4. mcp status label "missing" → "auth required" (minor)
5. dptree TypeId collision AgentDir/RightclawHome newtypes — already fixed in uncommitted changes

Scope: `config.rs`, `main.rs` (cmd_init, cmd_up, cloudflared generation), `codegen/cloudflared.rs`, `mcp/detect.rs`, `doctor.rs`, `bot/handler.rs`, `bot/dispatch.rs`. No new features.

</domain>

<decisions>
## Implementation Decisions

### Tunnel hostname storage (reverting Phase 36 D-01..D-06 decisions)
- **D-01:** `--tunnel-hostname <domain>` is required alongside `--tunnel-token` in `rightclaw init`. Both are required — no fallback to UUID URL. Validation fails with clear error if one is present without the other.
- **D-02:** `TunnelConfig` struct gets `hostname: String` field back. `config.yaml` stores both `token` and `hostname` under `tunnel:`. Phase 36 removed this field — it goes back in.
- **D-03:** `TunnelConfig::hostname()` method is removed. All call sites that used `tunnel_cfg.hostname()?` switch to direct field access `tunnel_cfg.hostname`. Bot healthcheck, redirect URI, cloudflared config generation all use the stored hostname.
- **D-04:** Add `TunnelConfig::tunnel_uuid() -> miette::Result<String>` — extracts UUID from token (same decode logic as the removed `hostname()` but returns just the UUID string, not `.cfargotunnel.com`). Used only for wrapper script generation in `cmd_up`. Keep single-segment + JWT format support.
- **D-05:** `cmd_init` prints derived UUID to stdout for confirmation: `"Tunnel UUID: <uuid> (hostname: <stored-hostname>)"`. Validates hostname is a bare domain/subdomain (no `https://` prefix) — fail fast with helpful error.

### DNS routing wrapper script
- **D-06:** `rightclaw up` generates a shell wrapper script at `~/.rightclaw/scripts/cloudflared-start.sh` before launching process-compose. Script content:
  ```sh
  #!/bin/sh
  set -e
  cloudflared tunnel route dns <UUID> <HOSTNAME>
  exec cloudflared tunnel run --token <TOKEN>
  ```
  `set -e` makes route dns failure fatal — cloudflared process does not start if DNS routing fails. process-compose restarts on failure.
- **D-07:** process-compose cloudflared process entry command: `~/.rightclaw/scripts/cloudflared-start.sh`. Script is chmod +x after write.
- **D-08:** UUID for the wrapper script is extracted via `tunnel_cfg.tunnel_uuid()?` in `cmd_up` during the cloudflared setup block. Fails `rightclaw up` if token is undecipherable.

### Doctor checks
- **D-09:** Doctor adds a new check: "tunnel-token" — calls `tunnel_cfg.tunnel_uuid()` and validates the result. Severity Warn if token cannot be decoded (reports "tunnel token invalid — cannot extract tunnel UUID"). Only runs if tunnel is configured. Existing cloudflared binary + tunnel-configured checks remain.
- **D-10:** DNS routing check and cloudflared process-alive check are left to Claude's Discretion — wrapper script failure is surfaced via process-compose restart behavior; explicit DNS check deferred.

### Bot healthcheck
- **D-11:** `handle_mcp_auth` line ~389 switches from `tunnel.hostname()` (UUID-derived) to `global_config.tunnel.hostname` (stored field). Same for redirect URI construction. Eliminates the root cause of the UAT test 10 failure.

### MCP tracing logs
- **D-12:** Add `tracing::info!` at the entry of each mcp subcommand handler: `handle_mcp_list`, `handle_mcp_auth`, `handle_mcp_add`, `handle_mcp_remove`. Log: command name + agent_dir. Example: `tracing::info!(agent_dir = %agent_dir.display(), "mcp list")`.

### rightclaw up warning visibility
- **D-13:** Line ~591 in `main.rs`: `tracing::warn!("MCP auth required: {}", auth_issues.join(", "))` → `eprintln!("warn: MCP auth required: {}", auth_issues.join(", "))`. `eprintln!` writes to stderr and is visible before TUI takes over stdout.

### mcp status labels
- **D-14:** `AuthState::Missing` Display impl: `"auth required"` (was `"missing"`). Test assertions that check for `"missing"` string must be updated to `"auth required"`.

### Already done (uncommitted, do not redo)
- `AgentDir(PathBuf)` and `RightclawHome(PathBuf)` newtypes in `handler.rs` + `dispatch.rs` — dptree collision fixed
- `TunnelConfig::hostname()` supports single-segment + JWT token formats
- `generate_cloudflared_config` param renamed `tunnel_url` → `tunnel_hostname` + scheme stripping
- Jinja2 template variable `tunnel_url` → `tunnel_hostname`

### Claude's Discretion
- Exact test coverage for wrapper script generation (unit test vs integration)
- Whether `cloudflared-start.sh` is regenerated on every `rightclaw up` or only when config changes
- Tracing log level for mcp handler entry (info vs debug)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### UAT gaps (source of truth for what to fix)
- `.planning/phases/v3.2-mcp-oauth-uat/v3.2-UAT.md` — 5 gaps listed under "Gaps" section, root_cause and artifacts per gap

### Core files to modify
- `crates/rightclaw/src/config.rs` — `TunnelConfig` struct (add hostname field back, add `tunnel_uuid()`, remove `hostname()`)
- `crates/rightclaw-cli/src/main.rs` — `Commands::Init` args (add --tunnel-hostname), `cmd_init` validation, `cmd_up` cloudflared block (wrapper script write, tunnel_uuid call), line ~591 eprintln fix
- `crates/rightclaw/src/codegen/cloudflared.rs` — already partially fixed (uncommitted); verify call sites pass stored hostname
- `crates/rightclaw/src/mcp/detect.rs` — `AuthState::Missing` Display: "auth required"
- `crates/bot/src/telegram/handler.rs` — tracing::info! in mcp handlers, tunnel.hostname field access in handle_mcp_auth
- `crates/bot/src/telegram/dispatch.rs` — already fixed (AgentDir/RightclawHome newtypes, uncommitted)
- `crates/rightclaw/src/doctor.rs` — add tunnel-token validity check (D-09)

### No external specs
Requirements fully captured in UAT gaps + decisions above.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `TunnelConfig::hostname()` decode logic (currently uncommitted in config.rs) — reuse as `tunnel_uuid()` with minor change (return UUID string, not `<uuid>.cfargotunnel.com`)
- `base64::engine::general_purpose::URL_SAFE_NO_PAD` + `serde_json::Value` — already used for token decode, keep as-is
- `eprintln!` already used at line ~621 for "no agents have Telegram tokens" — same pattern for MCP warning

### Established Patterns
- Doctor checks: `DoctorCheck { name, severity, status, message, fix }` struct — add new check following same pattern
- process-compose process entry `command` field: already string in codegen — change to script path
- `rightclaw up` writes cloudflared config YAML before launch — same pattern for writing wrapper script
- `tracing::info!(?key, ...)` style already used in `handle_message` and `handle_reset` — follow same style for mcp handlers

### Integration Points
- `cmd_up` at line ~636: `generate_cloudflared_config(&agent_pairs, &tunnel_cfg.hostname()?)` → `generate_cloudflared_config(&agent_pairs, &tunnel_cfg.hostname)` (direct field, no `?`)
- `handle_mcp_auth` at line ~389: `tunnel.hostname()` → read GlobalConfig and use `tunnel_cfg.hostname`
- process-compose YAML cloudflared `command` field: `cloudflared tunnel run --token {token}` → `{script_path}`

</code_context>

<specifics>
## Specific Ideas

- User confirmed: `--tunnel-hostname` required (not optional). Phase 36 decision to remove it is reversed by this phase.
- User confirmed: wrapper script is the approach for DNS routing, with `set -e` making it fatal.
- User confirmed: mcp status label → "auth required" (action-oriented over descriptive).

</specifics>

<deferred>
## Deferred Ideas

- Per-agent tunnel hostname — one tunnel URL per agent instead of shared — SEED territory
- DNS resolution check in doctor (verify CNAME exists) — left to Claude's Discretion per D-10
- cloudflared process-alive check via process-compose REST API in doctor — deferred past Phase 37

</deferred>

---

*Phase: 37-fix-v3-2-uat-gaps-tunnel-setup-flow-tunnel-hostname-dns-rout*
*Context gathered: 2026-04-04*
