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
  │   ├─ YES: resolve sandbox id + all sandbox-visible host IPs
  │   └─ NO: startup exits; creating a missing sandbox is an init/migration job
  ├─ Regenerate policy with exact host IPs
  │   ├─ host.openshell.internal resolved inside sandbox via getent ahosts
  │   ├─ IPv4 entries become /32; IPv6 entries become /128
  │   ├─ filesystem policy drift: write policy.yaml and fail startup with migration guidance
  │   └─ no filesystem drift: write policy.yaml and hot-apply via openshell policy set --wait
  ├─ generate_ssh_config (on every startup, host-side file)
  ├─ initial_sync (blocking — before teloxide starts)
  │   ├─ Deploy platform files to /sandbox/.platform/ (content-addressed + symlinks)
  │   ├─ Remove obsolete legacy built-in skill links from /sandbox/.claude/skills/
  │   └─ Download .claude.json, verify trust keys, fix if CC overwrote them
  └─ Background sync (every 5 min, re-deploys /sandbox/.platform/, GC stale entries)

Sandbox creation (`right init`, `right agent init`):
  ├─ prepare_staging_dir
  ├─ generate bootstrap policy with unresolved host.openshell.internal
  ├─ ensure_sandbox
  │   ├─ spawn_sandbox
  │   ├─ wait_for_ready
  │   └─ wait_for_ssh
  ├─ resolve host.openshell.internal inside sandbox
  ├─ hot-apply exact Right MCP allowed_ips via openshell policy set --wait
  └─ generate_ssh_config

Sandbox migration (`right agent config` filesystem-policy drift):
  ├─ ssh_tar_download old sandbox backup
  ├─ prepare_staging_dir
  ├─ spawn_sandbox with bootstrap policy → wait_for_ready → wait_for_ssh
  ├─ resolve host.openshell.internal inside new sandbox
  ├─ hot-apply exact Right MCP allowed_ips via openshell policy set --wait
  ├─ generate_ssh_config for new sandbox
  ├─ ssh_tar_upload backup into new sandbox
  └─ update agent.yaml, tear down old ControlMaster, delete old sandbox

Sandbox network:
  ├─ HTTP CONNECT proxy at 10.200.0.1:3128 (set via HTTPS_PROXY env)
  ├─ TLS MITM: L7 endpoints use TLS auto-detect (ClientHello peek) and
  │   termination for credential injection (OpenShell v0.0.30+)
  │   └─ Sandbox trusts CA via /etc/openshell-tls/ca-bundle.pem
  ├─ Permissive public web uses hostless public allowed_ips raw tunnels
  │   (`tls: skip`, no protocol/access) on 80/443 so scoped npm metadata
  │   paths containing `%2F` are not rejected by L7 request-target parsing
  └─ Policy controls which domains/IPs are allowed (wildcards supported for scoped public hosts)

Right MCP host access:
  ├─ mcp.json points OpenShell agents at http://host.openshell.internal:<port>/mcp
  ├─ First-create policy omits guessed Right MCP allowed_ips because the sandbox does not exist yet
  ├─ After READY, Right resolves host.openshell.internal from inside that exact sandbox
  ├─ Final policy includes every resolved IP as exact IPv4 /32 or IPv6 /128
  └─ openshell forward/service are not used for Right MCP; they expose sandbox services outward

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

The dashboard may run explicit, read-only OpenShell probes when the user opens
Mini App health, identity, or knowledge-skill views. These probes reuse the
bot-owned `SandboxExec` handle, are bounded by short per-request timeouts, and
read only the requested dashboard material: sandbox stats, identity files, or
skill inventory/details. Overview rendering does not run these probes.

Sandbox supervisors inject provider env-var placeholders at boot from the gateway's
attached-providers list (see `docs/architecture/providers.md`). The gateway proxy
substitutes the real credential on outbound HTTPS for TLS-terminated endpoints; raw
tunnels (tls: skip) cannot substitute and Right refuses to attach generic providers
against those hosts.

### User-Local CLI Environment

For OpenShell agents, Right Agent treats `/sandbox/.local/bin` as the canonical
user-installed executable directory. Startup sync writes
`/sandbox/.right/env.sh`, ensures `/sandbox/.bashrc` sources it, and the Claude
invocation wrapper sources the same file with an inline fallback. This makes
manually installed CLIs and `npm install -g` bins available both to `claude -p`
turns and to `right agent ssh` shells.

The managed environment sets `NPM_CONFIG_PREFIX=/sandbox/.local` and
`NPM_CONFIG_CACHE=/sandbox/.npm`. Agents should not use `sudo` or `~/bin` for
sandbox tool installs.
