---
status: complete
phase: 39-cloudflared-auto-tunnel
source: [39-01-SUMMARY.md]
started: 2026-04-05T00:00:00Z
updated: 2026-04-05T14:42:00Z
---

## Current Test

[testing complete]

## Tests

### 1. cert.pem absent — graceful skip
expected: Run `rightclaw init` when `~/.cloudflared/cert.pem` is absent. Should print an info message and complete without error — no tunnel configured, no crash.
result: issue
reported: "ran rightclaw init, was prompted for tunnel host, entered it, but cloudflared not visible in process-compose"
severity: major
note: "partial: init flow ran and prompted correctly (cert.pem present on machine). Cloudflared absent from PC is pre-existing deferred gap (main.rs:733 `let _ = cloudflared_script_path; // used by process-compose template in future phases`). Not a Phase 39 regression."

### 2. Help output — old arg gone
expected: Run `rightclaw init --help`. Output should show `--tunnel-name` and `--tunnel-hostname`. The old `--tunnel-credentials-file` must NOT appear.
result: pass

### 3. Non-interactive init with -y
expected: Run `rightclaw init --tunnel-name rightclaw --tunnel-hostname <yourhostname> -y` when cert.pem is present. Should complete without any interactive prompts and print a success message.
result: issue
reported: "it asked to reuse existing tunnel then asked for hostname"
severity: major

### 4. -y without --tunnel-hostname errors clearly
expected: Run `rightclaw init -y` when cert.pem is present but `--tunnel-hostname` is omitted. Should exit with a clear error message: "--tunnel-hostname is required when using -y" (or similar).
result: pass

### 5. Doctor fix hint updated
expected: Trigger the tunnel credentials doctor check (e.g., point config to a non-existent credentials file or run `rightclaw doctor` with a tunnel config whose credentials file is missing). The fix hint should say `--tunnel-name NAME --tunnel-hostname HOSTNAME`, NOT `--tunnel-credentials-file PATH`.
result: pass

## Summary

total: 5
passed: 3
issues: 2
pending: 0
skipped: 0
blocked: 0

## Gaps

- truth: "rightclaw init --tunnel-name X --tunnel-hostname Y -y completes without interactive prompts"
  status: failed
  reason: "User reported: it asked to reuse existing tunnel then asked for hostname"
  severity: major
  test: 3
  root_cause: "prompt_telegram_token() at main.rs:246 is called unconditionally — no yes-flag guard — so -y without --telegram-token always prompts; tunnel reuse/hostname prompts are gated correctly in current code and their appearance suggests a stale binary was tested"
  artifacts:
    - path: "crates/rightclaw-cli/src/main.rs"
      issue: "lines 241-246: match telegram_token has no None if yes => None branch; prompt_telegram_token() fires unconditionally when --telegram-token omitted regardless of -y"
  missing:
    - "Add None if yes => None arm to the telegram_token match before the None => prompt_telegram_token()? arm"

- truth: "After rightclaw init configures a tunnel, cloudflared runs as a process in process-compose"
  status: failed
  reason: "User reported: cloudflared not visible in process-compose after rightclaw init"
  severity: major
  test: 1
  root_cause: "Pre-existing deferred gap — cloudflared_script_path generated in cmd_up (main.rs:686) but intentionally not wired into process-compose template (main.rs:733: `let _ = cloudflared_script_path; // used by process-compose template in future phases`). process-compose.yaml.j2 has no cloudflared process entry."
  artifacts:
    - path: "crates/rightclaw-cli/src/main.rs"
      issue: "line 733: cloudflared_script_path unused, not passed to PC template"
    - path: "templates/process-compose.yaml.j2"
      issue: "no cloudflared process block"
  missing:
    - "Wire cloudflared_script_path into process-compose.yaml.j2 template as a process entry"
