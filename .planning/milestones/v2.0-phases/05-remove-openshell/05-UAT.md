---
status: complete
phase: v2.0 (phases 05-07 combined)
source: [05-01-SUMMARY.md, 05-02-SUMMARY.md, 06-01-SUMMARY.md, 06-02-SUMMARY.md, 07-01-SUMMARY.md, 07-02-SUMMARY.md]
started: 2026-03-24T16:00:00Z
updated: 2026-03-24T16:10:00Z
---

## Current Test

[testing complete]

## Tests

### 1. rightclaw init generates sandbox settings.json
expected: `rightclaw init` creates `.claude/settings.json` with sandbox.enabled, filesystem restrictions, network allowlist, secure defaults
result: pass
notes: Settings.json generated with all expected fields. allowWrite points to absolute agent dir path. 6 default domains present. denyRead for ~/.ssh, ~/.aws, ~/.gnupg.

### 2. rightclaw doctor checks bwrap/socat on Linux
expected: `rightclaw doctor` shows bwrap and socat checks with FAIL when not in PATH
result: pass
notes: Doctor correctly detects missing bwrap/socat with install guidance (apt/dnf/pacman).

### 3. rightclaw doctor does NOT check for openshell
expected: Doctor output has no openshell mention
result: pass
notes: 5 binary checks + agent validation. Zero openshell.

### 4. bwrap smoke test works when available
expected: `bwrap --ro-bind / / --unshare-net --dev /dev true` succeeds on this system
result: pass
notes: Tested via `nix run nixpkgs#bubblewrap`. AppArmor not blocking.

### 5. --no-sandbox generates sandbox.enabled=false
expected: settings.json has sandbox.enabled=false when --no-sandbox flag used
result: pass
notes: Tested via `rightclaw up --no-sandbox` in devenv. settings.json confirmed sandbox.enabled=false.

### 6. Shell wrapper launches claude directly
expected: Generated wrapper has `exec "$CLAUDE_BIN"` with no openshell
result: pass
notes: Wrapper at run/right.sh confirmed — direct exec, --dangerously-skip-permissions, --model, startup prompt.

### 7. rightclaw down does not destroy sandboxes
expected: Just stops process-compose, no sandbox destroy
result: pass
notes: Output: "All agents stopped." No sandbox destroy attempt.

### 8. agent.yaml sandbox overrides parsed correctly
expected: agent.yaml with sandbox: section parses without error
result: pass
notes: Tested with allow_write, allowed_domains fields. `rightclaw list` shows "config: yes", exit 0.

### 9. devenv includes sandbox dependencies
expected: devenv.nix includes bubblewrap and socat for development
result: pass
notes: Fixed — bubblewrap and socat added to devenv.nix. Doctor shows bwrap ok, socat ok, bwrap-sandbox ok.

### 10. Sandbox blocks filesystem writes outside agent dir
expected: Agent running via `rightclaw up` cannot write to `/tmp/should-fail` or any path outside its own agent directory.
result: pass
notes: `touch /tmp/should-fail` returned exit code 1 — "Read-only file system". Sandbox correctly enforces filesystem isolation.

### 11. Sandbox allows filesystem writes inside agent dir
expected: Agent can write files inside its own directory.
result: pass
notes: `touch test-file` created file at agent cwd (`agents/right/test-file`). Exit code 0.

### 12. Sandbox blocks network access to non-allowed domains
expected: Agent cannot reach domains not in allowedDomains.
result: pass
notes: `curl -s https://httpbin.org/get` timed out — proxy enforces allowlist, CONNECT tunnel hangs for disallowed domains.

### 13. Sandbox allows network access to allowed domains
expected: Agent can reach domains in allowedDomains.
result: pass
notes: `curl -s https://api.anthropic.com` returned Anthropic API banner. Exit code 0.

## Summary

total: 13
passed: 13
issues: 0
pending: 0
skipped: 0
blocked: 0

## Gaps

- truth: "devenv.nix includes bubblewrap and socat for development"
  status: resolved
  reason: "Fixed — bubblewrap and socat added to devenv.nix (cfeb289)"
  severity: minor
  test: 9
