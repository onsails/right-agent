# Deferred Items - Phase 06

## Pre-existing Issues (Out of Scope)

### Flaky init tests with parallel execution
- **Found during:** 06-01 Task 1 verification
- **Issue:** init tests (init_creates_default_agent_files, init_with_telegram_creates_settings_json, init_without_telegram_creates_settings_without_plugin) sporadically fail when run in parallel. They read ~/.claude/settings.json from the real filesystem, causing race conditions.
- **Workaround:** All tests pass with `--test-threads=1`
- **Root cause:** Tests depend on real filesystem state at ~/.claude/settings.json instead of using isolated temp directories for all paths
- **Not caused by:** Phase 06 changes (failures appear with unmodified code too)
