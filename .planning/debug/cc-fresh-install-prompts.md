---
status: awaiting_human_verify
trigger: "After rm -rf ~/.rightclaw && rightclaw init && rightclaw up, CC shows theme picker and asks for auth"
created: 2026-03-26T00:00:00Z
updated: 2026-03-26T00:00:00Z
---

## Current Focus

hypothesis: CONFIRMED -- generate_agent_claude_json only writes hasTrustDialogAccepted but not hasCompletedOnboarding, causing CC to show the theme picker (first-run onboarding). Auth prompt is likely part of the onboarding flow.
test: add hasCompletedOnboarding: true to agent-local .claude.json
expecting: CC should skip theme picker and auth prompt, launch directly into agent mode
next_action: implement fix in claude_json.rs

## Symptoms

expected: CC launches directly into agent mode, no theme picker, no auth prompt. Uses host user's existing OAuth credentials.
actual: CC shows "Let's get started" theme picker, then after selecting theme asks for authentication. OAuth token not picked up.
errors: No crash errors -- just unexpected interactive prompts in what should be headless agent launch.
reproduction: rm -rf ~/.rightclaw && cargo run --release --bin rightclaw -- init --telegram-token TOKEN --telegram-user-id ID && cargo run --release --bin rightclaw -- up --debug
started: Happens on fresh init. The codebase has logic to pre-trust agents and configure .claude/ settings but something is missing or broken.

## Eliminated

## Evidence

- timestamp: 2026-03-26T00:10:00Z
  checked: host ~/.claude.json keys
  found: hasCompletedOnboarding=true, lastOnboardingVersion="1.0.117", oauthAccount={...} all present
  implication: CC uses hasCompletedOnboarding in .claude.json to skip onboarding flow

- timestamp: 2026-03-26T00:11:00Z
  checked: agent ~/.rightclaw/agents/right/.claude.json
  found: only has projects.hasTrustDialogAccepted and cached feature flags from previous CC run. NO hasCompletedOnboarding key.
  implication: On fresh init, CC sees no hasCompletedOnboarding and shows theme picker + auth as onboarding flow

- timestamp: 2026-03-26T00:12:00Z
  checked: credential symlink at agent .claude/.credentials.json
  found: symlink exists and points to /home/wb/.claude/.credentials.json correctly
  implication: OAuth creds ARE accessible -- auth prompt is part of onboarding flow, not a missing credential issue

- timestamp: 2026-03-26T00:13:00Z
  checked: generate_agent_claude_json in claude_json.rs
  found: only writes hasTrustDialogAccepted under projects key, nothing about onboarding state
  implication: root cause confirmed -- need to add hasCompletedOnboarding: true

## Resolution

root_cause: generate_agent_claude_json() only writes hasTrustDialogAccepted to agent-local .claude.json but does not set hasCompletedOnboarding: true. When CC launches with HOME overridden to the agent dir, it reads this .claude.json, sees no onboarding completion flag, and triggers the first-run experience (theme picker -> auth prompt).
fix: Added hasCompletedOnboarding: true to generate_agent_claude_json() using or_insert (preserves existing value on re-runs). Added 2 tests: test_sets_onboarding_completed, test_does_not_overwrite_existing_onboarding.
verification: all 10 claude_json tests pass, all 15 init tests pass, workspace builds clean
files_changed: [crates/rightclaw/src/codegen/claude_json.rs]
