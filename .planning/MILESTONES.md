# Milestones

## v2.1 Headless Agent Isolation (Shipped: 2026-03-25)

**Phases completed:** 3 phases, 5 plans, 10 tasks

**Key accomplishments:**

- Shell wrapper sets HOME to agent dir with git/SSH/API key forwarding; per-agent .claude.json trust generation and credential symlink wired into cmd_up and init
- Absolute denyRead paths via host_home parameter, allowRead for agent dir, SandboxOverrides.allow_read, and integration tests covering Plan 01 artifacts
- Extended AgentConfig with three Telegram Option fields and extracted two codegen functions (generate_telegram_channel_config, install_builtin_skills) with 14 new tests covering all behaviors
- Wired git init, Telegram channel config, built-in skills reinstall, and settings.local.json pre-creation into cmd_up per-agent loop, and added git Warn check to doctor
- `rightclaw config strict-sandbox` writes /etc/claude-code/managed-settings.json with `allowManagedDomainsOnly:true`; doctor warns when file exists with rich or generic detail depending on content

---

## v2.0 Native Sandbox & Agent Isolation (Shipped: 2026-03-24)

**Phases completed:** 3 phases, 6 plans, 10 tasks

**Key accomplishments:**

- Stripped all OpenShell code paths -- sandbox.rs replaced by state.rs, policy.yaml removed from init/discovery/doctor, shell wrapper uses single direct-claude path
- v1 backward compatibility test added, all 48 relevant tests pass with zero openshell/sandbox references in codebase
- generate_settings() producing per-agent sandbox JSON with filesystem/network restrictions, security denyRead defaults, and user override merging via SandboxOverrides
- Wired generate_settings() into cmd_up() per-agent loop and refactored init.rs to delegate to shared codegen -- single source of truth for .claude/settings.json
- Linux-specific bwrap/socat binary detection and bwrap smoke test with AppArmor diagnostics in rightclaw doctor
- Replace OpenShell installation with bubblewrap + socat Linux deps and macOS Seatbelt early-return in install.sh

---
