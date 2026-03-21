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
