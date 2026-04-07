@CLAUDE.rust.md

This is a Rust project. Follow conventions in CLAUDE.rust.md.

<!-- GSD:project-start source:PROJECT.md -->
## Project

**RightClaw**

RightClaw is a multi-agent runtime for Claude Code built on NVIDIA OpenShell. Each agent runs as an independent Claude Code session inside its own OpenShell sandbox with declarative YAML policies. The Rust CLI orchestrates agent lifecycles via process-compose. Drop-in compatible with the OpenClaw/ClawHub ecosystem — same file conventions, same skill format, same registry — but with security-first enforcement instead of "grant all, pray it works."

**Core Value:** Run multiple autonomous Claude Code agents safely — each sandboxed by OpenShell policies, each with its own identity and memory, orchestrated by a single CLI command.

### Constraints

- **Language**: Rust (edition 2024)
- **Dependencies**: process-compose (external), OpenShell (external), Claude Code CLI (external)
- **Platforms**: Linux and macOS
- **Compatibility**: Drop-in compatible with OpenClaw file conventions and ClawHub SKILL.md format
- **Security**: Every agent must run inside OpenShell sandbox — no `--dangerously-skip-permissions` without policy enforcement
- **OpenShell status**: Alpha software — may have breaking changes. Design for resilience.
<!-- GSD:project-end -->

<!-- GSD:stack-start source:research/STACK.md -->
## Technology Stack

## Async vs Sync Decision
## Recommended Stack
### Core Framework
| Technology | Version | Purpose | Why | Confidence |
|------------|---------|---------|-----|------------|
| clap | 4.6.0 | CLI argument parsing | Industry standard. Derive API is ergonomic. Powers ripgrep, fd, bat. Subcommand model maps perfectly to `rightclaw up/down/status/attach/restart`. | HIGH |
| tokio | 1.50.0 | Async runtime | Required by reqwest. Process monitoring and HTTP calls benefit from async. Use `features = ["full"]` for process spawning + signal handling. | HIGH |
| serde | 1.0.228 | Serialization framework | Non-negotiable for any Rust project touching structured data. | HIGH |
### YAML Parsing
| Technology | Version | Purpose | Why | Confidence |
|------------|---------|---------|-----|------------|
| serde-saphyr | 0.0.22 | YAML deserialization | `serde_yaml` is deprecated (archived March 2024). `serde-saphyr` is the best replacement: panic-free on malformed input, no unsafe code, 1000+ passing tests including full yaml-test-suite, outperforms alternatives in benchmarks. The `serde_yml` fork (0.0.12) has lower adoption and less rigorous testing. | MEDIUM |
### Template Generation
| Technology | Version | Purpose | Why | Confidence |
|------------|---------|---------|-----|------------|
| minijinja | 2.18.0 | Generate process-compose.yaml | Jinja2-compatible, minimal dependencies (only serde), actively maintained by Armin Ronacher. Used by crates.io itself. Perfect for generating YAML configs from agent directory scan results. Avoids string concatenation for YAML generation. | HIGH |
### HTTP Client
| Technology | Version | Purpose | Why | Confidence |
|------------|---------|---------|-----|------------|
| reqwest | 0.13.2 | HTTP client for process-compose REST API and ClawHub API | 300M+ downloads. Async-first, ergonomic, handles JSON/headers/auth. Only serious choice for async Rust HTTP. | HIGH |
### Error Handling
| Technology | Version | Purpose | Why | Confidence |
|------------|---------|---------|-----|------------|
| thiserror | 2.0.18 | Structured error types | Derive macro for library-quality error enums. Use for internal error types (AgentError, PolicyError, ConfigError). | HIGH |
| miette | 7.6.0 | User-facing diagnostic errors | Compiler-style error output with source snippets, labels, suggestions. When a user's `agent.yaml` has invalid config, miette shows exactly where. Better UX than color-eyre for config-heavy CLIs. | HIGH |
### Process Management
| Technology | Version | Purpose | Why | Confidence |
|------------|---------|---------|-----|------------|
| std::process::Command | stdlib | Spawn process-compose and openshell | Standard library is sufficient for launching external processes. No need for exotic crates. | HIGH |
| tokio::process | 1.50.0 (via tokio) | Async process spawning/monitoring | Async `Child` with `kill_on_drop`. Use for launching `process-compose up` and monitoring its lifecycle. | HIGH |
| ctrlc | 3.5.2 | Signal handling (SIGINT/SIGTERM) | Cross-platform Ctrl+C handling. Triggers graceful shutdown: sends `process-compose down` before exit. Simple API, well-maintained. | HIGH |
### Logging/Tracing
| Technology | Version | Purpose | Why | Confidence |
|------------|---------|---------|-----|------------|
| tracing | 0.1.44 | Structured logging | Ecosystem standard. Structured spans for "agent-x starting", "policy validation", etc. | HIGH |
| tracing-subscriber | 0.3.23 | Log output formatting | `fmt` subscriber with `EnvFilter` for `RUST_LOG` support. Use `json` feature for machine-readable logs in detached mode. | HIGH |
### Testing
| Technology | Version | Purpose | Why | Confidence |
|------------|---------|---------|-----|------------|
| assert_cmd | 2.2.0 | CLI integration testing | Run `rightclaw` binary, assert on stdout/stderr/exit codes. The standard for Rust CLI testing. | HIGH |
| predicates | 3.1.4 | Assertion helpers | Pairs with assert_cmd. `predicate::str::contains("agent started")` style assertions. | HIGH |
| tempfile | (latest) | Temp directories for test agent layouts | Create isolated agent directory structures per test. | HIGH |
### Supporting Libraries
| Library | Version | Purpose | When to Use | Confidence |
|---------|---------|---------|-------------|------------|
| dirs | latest | XDG/platform directories | Locate config dirs, cache dirs for RightClaw state. | HIGH |
| which | latest | Find executables in PATH | Verify `process-compose`, `openshell`, `claude` are installed before `rightclaw up`. | HIGH |
| walkdir | latest | Directory traversal | Scan `agents/` directory to discover agent subdirectories. | HIGH |
## External Dependencies (Not Rust Crates)
| Technology | Version | Purpose | Integration Pattern |
|------------|---------|---------|---------------------|
| process-compose | v1.100.0+ | Process orchestration + TUI | RightClaw generates `process-compose.yaml`, launches `process-compose up`. Controls via REST API on `localhost:8080` (or UDS). Auth via `PC_API_TOKEN`. |
| OpenShell | latest (alpha) | Sandbox enforcement | CLI invocation: `openshell sandbox create --policy <path> -- claude`. Each agent process in process-compose.yaml wraps its command with openshell. |
| Claude Code CLI | latest | AI agent sessions | Launched inside OpenShell sandboxes. Not directly managed by RightClaw -- process-compose handles lifecycle. |
## Integration Patterns
### process-compose Integration
### OpenShell Integration
# Per-agent command in process-compose.yaml:
# Policy hot-reload (network/inference sections):
## Alternatives Considered
| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| YAML parsing | serde-saphyr | serde_yml | Less rigorous testing, simple fork of deprecated crate |
| YAML parsing | serde-saphyr | serde_yaml | Deprecated and archived since March 2024 |
| Template engine | minijinja | tera | Heavier deps, diverges from Jinja2, overkill for YAML generation |
| Template engine | minijinja | string formatting | Error-prone for nested YAML structures |
| HTTP client | reqwest | ureq | Sync-only, conflicts with async architecture |
| Error handling | miette + thiserror | color-eyre | color-eyre archived, miette better for config-error-heavy CLI |
| Error handling | miette + thiserror | anyhow | Too generic, no structured error types |
| Signal handling | ctrlc | signal-hook | Overpowered for SIGINT/SIGTERM only |
| CLI testing | assert_cmd | trycmd | Too rigid for early-stage development |
| Async runtime | tokio | smol/async-std | Ecosystem is tokio. reqwest needs tokio. No contest. |
## Cargo.toml Dependencies
## Sources
- [clap on crates.io](https://crates.io/crates/clap) - v4.6.0, verified 2026-03-21
- [reqwest on crates.io](https://crates.io/crates/reqwest) - v0.13.2, verified 2026-03-21
- [serde-saphyr on crates.io](https://crates.io/crates/serde-saphyr) - v0.0.22, verified 2026-03-21
- [minijinja on crates.io](https://crates.io/crates/minijinja) - v2.18.0, verified 2026-03-21
- [miette on crates.io](https://crates.io/crates/miette) - v7.6.0, verified 2026-03-21
- [thiserror on crates.io](https://crates.io/crates/thiserror) - v2.0.18, verified 2026-03-21
- [tokio on crates.io](https://crates.io/crates/tokio) - v1.50.0, verified 2026-03-21
- [ctrlc on crates.io](https://crates.io/crates/ctrlc) - v3.5.2, verified 2026-03-21
- [process-compose GitHub releases](https://github.com/F1bonacc1/process-compose/releases) - v1.100.0
- [process-compose remote client docs](https://f1bonacc1.github.io/process-compose/client/)
- [NVIDIA OpenShell GitHub](https://github.com/NVIDIA/OpenShell)
- [NVIDIA OpenShell Developer Guide](https://docs.nvidia.com/openshell/latest/index.html)
- [serde_yaml deprecation discussion](https://users.rust-lang.org/t/serde-yaml-deprecation-alternatives/108868)
- [Rust CLI testing with assert_cmd](https://alexwlchan.net/2025/testing-rust-cli-apps-with-assert-cmd/)
- [Async Rust: When to Use It](https://www.wyeworks.com/blog/2025/02/25/async-rust-when-to-use-it-when-to-avoid-it/)
<!-- GSD:stack-end -->

<!-- GSD:conventions-start source:CONVENTIONS.md -->
## Conventions

- **Bot-first management**: All agent/MCP configuration goes through the Telegram bot (`/mcp add`, `/mcp remove`, `/mcp auth`, etc.). Never create or edit `.mcp.json`, agent configs, or credential files manually — the bot is the control plane.
- **Debuggability over convenience**: Always prefer direct, observable signals over indirect heuristics. If an API provides status, use it — don't infer status from side effects (e.g. SSH connectivity as a proxy for sandbox readiness). Errors must propagate to logs, never be silently swallowed.
- **Domain research before implementation**: Always verify external tool APIs by reading source code or running `--help` before writing integration code. Never rely solely on web documentation — it may be outdated or wrong.
<!-- GSD:conventions-end -->

<!-- GSD:architecture-start source:ARCHITECTURE.md -->
## Architecture

Architecture not yet mapped. Follow existing patterns found in the codebase.
<!-- GSD:architecture-end -->

<!-- GSD:workflow-start source:GSD defaults -->
## GSD Workflow Enforcement

Before using Edit, Write, or other file-changing tools, start work through a GSD command so planning artifacts and execution context stay in sync.

Use these entry points:
- `/gsd:quick` for small fixes, doc updates, and ad-hoc tasks
- `/gsd:debug` for investigation and bug fixing
- `/gsd:execute-phase` for planned phase work

Do not make direct repo edits outside a GSD workflow unless the user explicitly asks to bypass it.
<!-- GSD:workflow-end -->



<!-- GSD:profile-start -->
## Developer Profile

> Profile not yet configured. Run `/gsd:profile-user` to generate your developer profile.
> This section is managed by `generate-claude-profile` -- do not edit manually.
<!-- GSD:profile-end -->
