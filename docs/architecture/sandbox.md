# OpenShell sandbox

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `CLAUDE.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## OpenShell Sandbox Architecture

Sandboxes are **persistent** — never deleted automatically. They live as long as the agent lives and survive bot restarts.

```
Bot startup:
  ├─ gRPC GetSandbox → exists?
  │   ├─ YES: apply_policy (hot-reload via openshell policy set --wait)
  │   └─ NO: prepare_staging_dir → spawn_sandbox → wait_for_ready
  ├─ generate_ssh_config (on every startup, host-side file)
  ├─ initial_sync (blocking — before teloxide starts)
  │   ├─ Deploy platform files to /sandbox/.platform/ (content-addressed + symlinks)
  │   └─ Download .claude.json, verify trust keys, fix if CC overwrote them
  └─ Background sync (every 5 min, re-deploys /sandbox/.platform/, GC stale entries)

Sandbox network:
  ├─ HTTP CONNECT proxy at 10.200.0.1:3128 (set via HTTPS_PROXY env)
  ├─ TLS MITM: proxy auto-detects TLS (ClientHello peek) and terminates
  │   unconditionally for credential injection (OpenShell v0.0.30+)
  │   └─ Sandbox trusts CA via /etc/openshell-tls/ca-bundle.pem
  └─ Policy controls which domains are allowed (wildcards supported)

Staging dir (minimal bootstrap — platform files deployed via /sandbox/.platform/ during initial_sync):
  ├─ .claude/settings.json    — CC behavioral flags
  ├─ .claude/reply-schema.json — structured output schema
  ├─ .claude.json              — trust + onboarding
  └─ mcp.json                  — MCP server entries
  EXCLUDED: skills (deployed to /sandbox/.platform/), credentials, plugins

Platform store (/sandbox/.platform/ inside sandbox):
  ├─ Content-addressed files: settings.json.<hash>, reply-schema.json.<hash>, ...
  ├─ Content-addressed skill dirs: skills/rightmcp.<hash>/, skills/rightcron.<hash>/
  ├─ Symlinked from /sandbox/.claude/ → /sandbox/.platform/
  ├─ Read-only (chmod a-w after deploy)
  └─ GC removes stale entries after each sync cycle
```
