# Phase 40: Wire Cloudflared into Process-Compose — Context

**Gathered:** 2026-04-05
**Status:** Ready for planning

<domain>
## Phase Boundary

When `TunnelConfig` is present in `~/.rightclaw/config.yaml`, `rightclaw up` adds cloudflared as a
process-compose entry alongside bot agents. The `cloudflared-start.sh` wrapper script is already
generated in `cmd_up` (Phase 38/39) but intentionally not wired into the PC template
(`main.rs:733: let _ = cloudflared_script_path; // used by process-compose template in future phases`).
This phase closes that gap — nothing else.
</domain>

<decisions>
## Implementation Decisions

### Cloudflared PC Process Config
- **D-01:** Process name: `cloudflared` (single entry, no suffix)
- **D-02:** Restart policy: `on_failure` — matches bot agents; restarts on crash, not on clean exit
- **D-03:** No `depends_on` from bot agents to cloudflared — bots work independently of the tunnel (Telegram polling doesn't need it); OAuth flows fail gracefully until tunnel is up
- **D-04:** Shutdown: signal 15 (SIGTERM), timeout_seconds 30 — same as bot agents
- **D-05:** Backoff/max_restarts: use hardcoded defaults (`backoff_seconds: 5`, `max_restarts: 10`) — cloudflared is not an AgentDef so no per-agent config applies

### Template Change
- **D-06:** Conditional block in `templates/process-compose.yaml.j2` — cloudflared process only rendered when a `cloudflared` context variable is present (truthy)
- **D-07:** `cloudflared.command` = absolute path to `cloudflared-start.sh` (script handles `route dns || true` then `exec cloudflared tunnel --config ... run`)

### API Change (`generate_process_compose`)
- **D-08:** Add `cloudflared_script: Option<&Path>` parameter to `generate_process_compose` — pass `None` when no tunnel config, pass script path when tunnel configured
- **D-09:** Remove `let _ = cloudflared_script_path;` in `cmd_up` and pass `cloudflared_script_path.as_deref()` to `generate_process_compose`

### Binary Pre-flight
- **D-10:** `cmd_up` fails fast with a clear error if `TunnelConfig` is present but `cloudflared` binary is not in PATH — checked via `which::which("cloudflared")` before PC config generation; better UX than watching the PC process restart repeatedly

### Claude's Discretion
- Working directory for the cloudflared process
- Exact process-compose availability fields structure in the template
</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Core files to change
- `crates/rightclaw-cli/src/main.rs` — `cmd_up`: cloudflared script generation (lines ~684-733); `let _ = cloudflared_script_path` placeholder to remove; binary check to add; pass script to codegen
- `crates/rightclaw/src/codegen/process_compose.rs` — `generate_process_compose` function signature and template context building
- `crates/rightclaw/src/codegen/process_compose_tests.rs` — existing tests; add tests for cloudflared case
- `templates/process-compose.yaml.j2` — add conditional cloudflared process block

### Supporting context
- `.planning/phases/39-cloudflared-auto-tunnel/39-UAT.md` — UAT gap that Phase 40 closes (test 1 note: "cloudflared not visible in process-compose")
- `.planning/REQUIREMENTS.md` §TUNL-01 — "rightclaw up spawns cloudflared as a persistent process-compose entry" (TUNL-02 merged into TUNL-01 per requirements note)
</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `cloudflared_script_path: Option<PathBuf>` — already built in `cmd_up` at lines ~686-732; script at `~/.rightclaw/scripts/cloudflared-start.sh`
- `which::which("cloudflared")` — already used in `cmd_init` (line ~269); reuse same pattern for pre-flight in `cmd_up`
- `BotProcessAgent` struct and template context pattern — extend for cloudflared; or just pass the script path as a separate template variable

### Established Patterns
- `generate_process_compose(agents, exe_path, debug)` → `generate_process_compose(agents, exe_path, debug, cloudflared_script)` — additive signature change
- Template conditionals via Jinja2 `{% if cloudflared %}...{% endif %}` — already used for `token_file` vs `token_inline`
- Process entry structure mirrors existing bot agent entries (command, working_dir, availability, shutdown)
- Tests in `process_compose_tests.rs` (separate file via `#[path = ...]`) — add 2 cases: with and without cloudflared script

### Integration Points
- `cmd_up` in `main.rs`: passes `cloudflared_script_path.as_deref()` to `generate_process_compose`
- `generate_process_compose` in `codegen/process_compose.rs`: renders template with optional cloudflared block
- Template in `templates/process-compose.yaml.j2`: conditional cloudflared process rendered after agents loop
</code_context>

<specifics>
## Specific Ideas

- "We use the same mode as before" — `on_failure` restart, matching existing bot agent policy
- "The script sets up routes on every process up and then starts a tunnel" — confirmed: `cloudflared-start.sh` runs `route dns || true` then `exec cloudflared tunnel --config ... run`; re-running DNS route on restart is intentional and non-fatal
</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.
</deferred>

---

*Phase: 40-wire-cloudflared-into-process-compose*
*Context gathered: 2026-04-05*
