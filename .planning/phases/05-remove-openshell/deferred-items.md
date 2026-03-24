# Deferred Items - Phase 05

## Pre-existing Test Failures (Out of Scope)

### 1. init::tests::init_with_telegram_creates_settings_json
- **File:** `crates/rightclaw/src/init.rs:438`
- **Issue:** `pre_trust_directory()` reads `~/.claude/settings.json` which may be empty, causing JSON parse error
- **Root cause:** `pre_trust_directory()` doesn't handle empty/missing settings.json gracefully
- **Impact:** Test fails when `~/.claude/settings.json` exists but is empty

### 2. init::tests::init_errors_if_already_initialized
- **Same root cause** as above -- test runs `init_rightclaw_home` which calls `pre_trust_directory()`

### 3. cli_integration::test_status_no_running_instance
- **File:** `crates/rightclaw-cli/tests/cli_integration.rs`
- **Issue:** Test expects "No running instance" in stderr but gets HTTP connection error instead
- **Root cause:** Error message format mismatch for status command when no process-compose is running
