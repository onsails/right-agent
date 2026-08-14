## Rust Project Standards

### 1. Dependency Versioning
- Always use requirements: `x.x` (ensures patch compatibility)
- Example: `serde = "1.0"`

### 2. Error Handling - FAIL FAST Principle

**Why This Matters:**
- Silent failures corrupt data and leave systems in undefined states
- Half-completed operations are worse than crashes (harder to debug, data inconsistency)
- Errors cascade: one swallowed error causes 10 mysterious failures downstream
- Logging without propagating gives false confidence that errors are "handled"

**The Rule:** Every error MUST propagate up the call stack. The program halts on errors.

**CORRECT - Always propagate errors:**
```rust
// Best: Use ? operator
operation()?;

// With context: Add context AND propagate
operation().context("failed during initialization")?;

// Log for observability AND propagate (both required!)
let result = operation().map_err(|e| {
    tracing::error!("Operation failed: {e}");
    e
})?;

// Explicit match when you need it
match operation() {
    Ok(val) => process(val),
    Err(e) => return Err(e.into()),
}
```

**FORBIDDEN - These all swallow errors:**
```rust
if let Err(e) = operation() { log::error!("{e}"); }  // No return!
operation().unwrap_or_default();  // Silent fallback
operation().ok();  // Discards error
let _ = operation();  // Explicitly ignores
```

**Self-Check:** If you see `if let Err` or `match ... Err` without `return Err` or `?`, it's a bug.

**NOT "Error Handling":** Adding logging is NOT fixing/handling an error. The error must propagate.

**Preserve Error Chains:** When converting `anyhow::Error` to `String` (for logging, wrapping in other error types, etc.), ALWAYS use `format!("{:#}", e)` (alternate Display). NEVER use `e.to_string()` or `format!("{}", e)` -- these show only the outermost context and hide the root cause.

### 3. Error Types
- **Library crates/modules**: Use `thiserror` with backtrace support
- **Binary main.rs & tests**: Use `anyhow`
- **Other derives**: Use `derive_more` (Display, From, Into, etc.)

### 4. Workspace Architecture
- Always use Cargo workspace with single-responsibility crates
- Root `Cargo.toml` defines workspace, contains no code
- CLI must be separate subcrate
- Structure: `project/`, `project-cli/`, `project-client/`, etc.

### 5. Testing
- **NEVER** use `std::env::set_var()` in tests (pollutes environment)
- **ALWAYS** pass config through function parameters
- **External integration tests**: tests requiring live OpenShell, Claude Code inside a sandbox, real ffmpeg/Whisper inference, or network model downloads must be `#[ignore]` locally and explicitly invoked by GitHub Actions jobs. Use stable ignore reasons and workspace-filterable test name prefixes together: `ci-openshell: ...` with `ci_openshell_`, `ci-claude: ...` with `ci_claude_`, `ci-stt: ...` with `ci_stt_`. `crates/right/tests/ci_ignored_contract.rs` enforces this so future packages are not missed.
- **Cadence**: TDD still applies, but use the narrowest useful command for the loop. Run the new/regression test first and verify it fails; after implementation rerun that test or the nearest package/module suite. Do not run full workspace tests after every edit or every small plan step.
- **Targeting**: Prefer `devenv shell -- cargo nextest run -p <crate> <filter>` or `devenv shell -- cargo nextest run -p <crate>` during development (`cargo test` still works but nextest is the recommended runner; doctests run only under `cargo test --doc`). Use workspace-wide tests midstream only for broad cross-crate changes or when targeted results cannot prove the behavior.
- **Worktrees**: At worktree start, run one baseline verification appropriate to the planned scope and record existing failures. At worktree completion, run the final full workspace test from inside that worktree.
- **Shared test sandbox**: live OpenShell I/O tests reuse one cross-process sandbox per runner invocation, named `rt-<label>-<runid>` (`runid` = `RIGHT_TEST_RUN_ID` or the runner's pid), fitted via `fit_sandbox_name` to the 19-char upstream routable-name cap (`MAX_SANDBOX_NAME_LEN`, OpenShell v0.0.86+). It is not deleted on test exit. CI runners are ephemeral so nothing accumulates there; locally, prune leftovers with `openshell sandbox list` then `openshell sandbox delete rt-...`. Never delete one mid-run — a different `runid` may be live.
- **Final verification**: Before declaring code work complete, run `devenv shell -- cargo nextest run --workspace` plus `devenv shell -- cargo test --doc --workspace`. This is mandatory even when all targeted tests passed.
- Tests in same file using `#[cfg(test)]` module
- **Large files**: If file exceeds 800 LoC and tests are >50% of content, extract tests to separate file:
  ```rust
  #[cfg(test)]
  #[path = "mymodule_tests.rs"]
  mod tests;
  ```
  Keep test file in same directory as source (e.g., `src/mymodule.rs` -> `src/mymodule_tests.rs`)

### 6. Configuration Management
- **CLI-First**: Never bypass CLI argument parsing
- **NEVER** use `Default` trait that reads environment
- **ALWAYS** use `from_cli_args()` factory methods
- Config flows: CLI args -> Config struct -> Client

### 7. Python Helper Scripts
- Location: `helpers/` directory
- Initialize: `uv init helpers/`
- **ALWAYS** use `uv add <package>` (NEVER `uv pip install`)

### 8. Code Standards
- **Visibility**: Private (default) > pub(crate) > pub
- **Magic Numbers**: Use `const` or CLI args, never literals
- **Async**: Use tokio consistently
- **Breaking Changes**: OK for internal crates, preserve HTTP/WebSocket compatibility

### 9. Rust Versioning
- **Cargo edition**: Use 2024 edition
