---
status: investigating
trigger: "cc-theme-prompt-on-launch: After rm -rf ~/.rightclaw && rightclaw init && rightclaw up --debug, Claude Code shows the first-run theme selection prompt instead of starting silently"
created: 2026-03-26T00:00:00Z
updated: 2026-03-26T00:00:00Z
---

## Current Focus

hypothesis: rightclaw init does not write CC's onboarding-complete state to ~/.claude/config.json (or equivalent), so CC sees a fresh profile and triggers first-run theme picker
test: find what file/key CC uses to track onboarding state, then check if rightclaw init/up writes it
expecting: missing write to ~/.claude/config.json or ~/.claude/.credentials.json with theme/onboarding keys
next_action: read init command source and find all files written to disk during init

## Symptoms

expected: Claude Code should start without any interactive prompts (theme, onboarding, trust dialogs)
actual: CC shows "Let's get started" with theme picker (Dark mode, Light mode, etc.)
errors: No errors - just unwanted interactive prompt
reproduction: rm -rf ~/.rightclaw && cargo run --release --bin rightclaw -- init --telegram-token TOKEN --telegram-user-id 85743491 && cargo run --release --bin rightclaw -- up --debug
started: Reproducible on fresh init

## Eliminated

(none yet)

## Evidence

(none yet)

## Resolution

root_cause:
fix:
verification:
files_changed: []
