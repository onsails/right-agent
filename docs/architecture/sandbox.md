# OpenShell sandbox

> **Status:** descriptive doc. Re-read and update when modifying this
> subsystem (see `AGENTS.md` → "Architecture docs split"). Code is
> authoritative; this file may have drifted.

## OpenShell Sandbox Architecture

Sandboxes are **persistent** — never deleted automatically. They live as long as the agent lives and survive bot restarts.

```
Bot startup:
  ├─ Resolve OpenShell gateway endpoint (OPENSHELL_GATEWAY_ENDPOINT or openshell status)
  ├─ gRPC GetSandbox → READY?
  │   ├─ YES: resolve sandbox id + host IP
  │   └─ NO: startup exits; creating a missing sandbox is an init/migration job
  ├─ Regenerate policy with resolved host IP
  │   ├─ filesystem policy drift: write policy.yaml, skip apply, warn to trigger migration
  │   └─ no filesystem drift: write policy.yaml and hot-apply via openshell policy set --wait
  ├─ generate_ssh_config (on every startup, host-side file)
  ├─ initial_sync (blocking — before teloxide starts)
  │   ├─ Deploy platform files to /sandbox/.platform/ (content-addressed + symlinks)
  │   ├─ Remove obsolete legacy built-in skill links from /sandbox/.claude/skills/
  │   └─ Download .claude.json, verify trust keys, fix if CC overwrote them
  └─ Background sync (every 5 min, re-deploys /sandbox/.platform/, GC stale entries)

Sandbox creation (`right init`, `right agent init`):
  ├─ prepare_staging_dir
  ├─ ensure_sandbox
  │   ├─ spawn_sandbox
  │   ├─ wait_for_ready
  │   └─ wait_for_ssh
  └─ generate_ssh_config

Sandbox migration (`right agent config` filesystem-policy drift):
  ├─ ssh_tar_download old sandbox backup
  ├─ prepare_staging_dir
  ├─ spawn_sandbox → wait_for_ready → wait_for_ssh
  ├─ generate_ssh_config for new sandbox
  ├─ ssh_tar_upload backup into new sandbox
  └─ update agent.yaml, tear down old ControlMaster, delete old sandbox

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
  ├─ mcp.json                  — MCP server entries
  └─ TOOLS.md                  — agent-editable tool notes seeded for turn 1
  EXCLUDED: skills (deployed to /sandbox/.platform/), credentials, plugins

Platform store (/sandbox/.platform/ inside sandbox):
  ├─ Content-addressed files: settings.json.<hash>, reply-schema.json.<hash>, ...
  ├─ Content-addressed skill dirs (one per `right_codegen::BUILTIN_SKILL_NAMES`):
  │     skills/right-skills.<hash>/, skills/right-cron.<hash>/, skills/right-mcp.<hash>/,
  │     skills/right-learn-skill.<hash>/, skills/right-memory.<hash>/,
  │     skills/right-reflect.<hash>/
  ├─ Symlinked from /sandbox/.claude/ → /sandbox/.platform/
  ├─ Read-only (chmod a-w after deploy)
  └─ GC removes stale entries after each sync cycle
```

Learned skill packages are agent-owned directories under
`/sandbox/.claude/skills/rightx-*`. The learning MCP tools do not patch
non-`rightx-*` skill directories and do not copy skill files from sandbox to
host.
