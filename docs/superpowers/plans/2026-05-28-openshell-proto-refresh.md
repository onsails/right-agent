# OpenShell proto refresh + provider gRPC migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-vendor OpenShell protos to v0.0.50, add a CLI+gateway version preflight, and migrate every provider control-plane call site from CLI to gRPC.

**Architecture:** Drop CLI argv plumbing in `right-openshell::providers` and replace each function body with a tonic RPC call. Provider functions take `&mut OpenShellClient<Channel>` (matches the existing `sandbox_exists`/`wait_for_deleted` style in `right-openshell::openshell`), so callers in `right::internal_api_providers` open one client per request and thread it. Add `openshell_preflight` to `crates/bot/src/lib.rs` startup that hard-fails on too-old CLI or gateway. Mock-gRPC unit tests live in `crates/right-openshell/src/providers_tests.rs` + `preflight_tests.rs` and run on every `cargo test --workspace`.

**Tech Stack:** Rust 2024, tonic 0.x (existing), prost (existing), `semver` (new dep on `right-openshell`), `thiserror` (existing), tokio (existing).

---

## File Structure

**Create:**
- `scripts/vendor-openshell-proto.sh` — vendor script (bash)
- `crates/right-openshell/proto/UPSTREAM.md` — pinned tag + fetch date
- `crates/right-openshell/src/preflight.rs` — `MIN_OPENSHELL_VERSION`, `PreflightError`, `openshell_preflight`, `parse_openshell_cli_version`
- `crates/right-openshell/src/preflight_tests.rs` — preflight unit tests
- `crates/right-openshell/src/providers_tests.rs` — provider RPC mock-server unit tests

**Re-vendor (overwritten by script):**
- `crates/right-openshell/proto/openshell/openshell.proto`
- `crates/right-openshell/proto/openshell/datamodel.proto`
- `crates/right-openshell/proto/openshell/sandbox.proto`

**Modify:**
- `crates/right-openshell/Cargo.toml` — add `semver`, expose preflight module
- `Cargo.toml` (workspace) — add `semver = "1.0"` to `[workspace.dependencies]`
- `crates/right-openshell/src/lib.rs` — add `pub mod preflight`
- `crates/right-openshell/src/providers.rs` — gRPC rewrite; signature change; delete CLI helpers
- `crates/right-openshell/src/openshell_tests.rs` — add provider RPC handlers + mock_client export
- `crates/right-openshell/tests/ci_openshell_provider.rs` — drop `ensure_v2_enabled` calls; update signatures
- `crates/right/src/internal_api_providers.rs` — open `OpenShellClient` once per request; pass `&mut client` to provider fns
- `crates/bot/src/lib.rs` — call `openshell_preflight` adjacent to `upgrade::check`
- `ARCHITECTURE.md` — proto-version + gotcha notes
- `docs/architecture/providers.md` — CLI→gRPC narrative
- `docs/architecture/sandbox.md` — preflight step in startup

---

## Task 1: Vendor script and pinned-tag marker

**Files:**
- Create: `scripts/vendor-openshell-proto.sh`
- Create: `crates/right-openshell/proto/UPSTREAM.md`

- [ ] **Step 1: Create the vendor script**

Write `scripts/vendor-openshell-proto.sh`:

```bash
#!/usr/bin/env bash
# Vendor OpenShell .proto files from a pinned upstream tag.
#
# Usage: scripts/vendor-openshell-proto.sh <tag>
# Example: scripts/vendor-openshell-proto.sh v0.0.50
#
# Re-pulls datamodel.proto, sandbox.proto, openshell.proto from
# https://raw.githubusercontent.com/NVIDIA/OpenShell/<tag>/proto/
# into crates/right-openshell/proto/openshell/ and writes the tag +
# fetch timestamp into crates/right-openshell/proto/UPSTREAM.md.
set -euo pipefail

TAG="${1:?usage: $0 <tag>  (e.g. v0.0.50)}"
DEST_DIR="crates/right-openshell/proto/openshell"
UPSTREAM_FILE="crates/right-openshell/proto/UPSTREAM.md"

if [[ ! -d "$DEST_DIR" ]]; then
    echo "error: $DEST_DIR not found; run from repo root" >&2
    exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

for f in datamodel.proto sandbox.proto openshell.proto; do
    url="https://raw.githubusercontent.com/NVIDIA/OpenShell/${TAG}/proto/${f}"
    echo "fetching $url"
    curl -fsSL "$url" -o "$TMP/$f"
done

rm -f "$DEST_DIR"/*.proto
mv "$TMP"/*.proto "$DEST_DIR/"

printf 'tag: %s\nfetched: %s\nupstream: https://github.com/NVIDIA/OpenShell\n' \
    "$TAG" "$(date -u +%FT%TZ)" > "$UPSTREAM_FILE"

echo "Vendored OpenShell proto from $TAG"
echo "Run: cargo check -p right-openshell  to regenerate tonic stubs"
```

- [ ] **Step 2: Make the script executable**

```bash
chmod +x scripts/vendor-openshell-proto.sh
```

- [ ] **Step 3: Run the script against v0.0.50**

```bash
./scripts/vendor-openshell-proto.sh v0.0.50
```

Expected:
- `fetching https://raw.githubusercontent.com/.../v0.0.50/proto/datamodel.proto`
- `fetching .../sandbox.proto`
- `fetching .../openshell.proto`
- `Vendored OpenShell proto from v0.0.50`

- [ ] **Step 4: Verify UPSTREAM.md was written**

```bash
cat crates/right-openshell/proto/UPSTREAM.md
```

Expected content shape:
```
tag: v0.0.50
fetched: 2026-05-28T...Z
upstream: https://github.com/NVIDIA/OpenShell
```

- [ ] **Step 5: Verify proto regeneration compiles the lib**

```bash
devenv shell -- cargo build -p right-openshell --lib
```

Expected: this may **fail**. v0.0.50 moved several messages out of `datamodel.proto` into `openshell.proto`:

| Type | v0.0.42 path | v0.0.50 path |
|---|---|---|
| `Sandbox` | `datamodel::v1::Sandbox` | `v1::Sandbox` |
| `SandboxStatus` | `datamodel::v1::SandboxStatus` | `v1::SandboxStatus` |
| `SandboxCondition` | `datamodel::v1::SandboxCondition` | `v1::SandboxCondition` |
| `ObjectMeta` | `datamodel::v1::ObjectMeta` | `datamodel::v1::ObjectMeta` (unchanged) |
| `Provider` | `datamodel::v1::Provider` | `datamodel::v1::Provider` (unchanged) |

Fix the import paths in any file the compiler points at. The candidates are:

```bash
grep -n "openshell_proto::openshell::datamodel::v1" crates/right-openshell/src/*.rs
```

Most likely files needing edits:
- `crates/right-openshell/src/openshell.rs` — imports `Sandbox`, possibly `SandboxStatus`
- `crates/right-openshell/src/openshell_tests.rs` — line ~600 imports `SandboxCondition, SandboxStatus`; line ~685 references `datamodel::v1::Sandbox`

For each error of the form `unresolved import: crate::openshell_proto::openshell::datamodel::v1::SandboxStatus`, change `datamodel::v1::` to `v1::` for that type. Repeat until the lib build is clean.

The trait `open_shell_server::OpenShell` will also now require several new methods (`attach_sandbox_provider`, `detach_sandbox_provider`, `list_sandbox_providers`, `expose_service`, etc.). Don't fix the test-side `impl OpenShell for MockOpenShell` here — that's deferred to Task 5. The lib itself (no tests) must compile.

- [ ] **Step 6: Verify the lib builds clean**

```bash
devenv shell -- cargo build -p right-openshell --lib
```

Expected: success after the path-fix loop above.

- [ ] **Step 7: Commit**

```bash
git add scripts/vendor-openshell-proto.sh \
        crates/right-openshell/proto/UPSTREAM.md \
        crates/right-openshell/proto/openshell/*.proto \
        crates/right-openshell/src/openshell.rs \
        crates/right-openshell/src/openshell_tests.rs
git commit -m "build(openshell): vendor protos from NVIDIA/OpenShell v0.0.50

Re-pulls datamodel.proto, sandbox.proto, openshell.proto from upstream
v0.0.50 via the new scripts/vendor-openshell-proto.sh, replacing the
April 2026 snapshot that predates gateway providers. UPSTREAM.md
records the pinned tag and fetch timestamp.

Fixes the import-path ripple from v0.0.50 moving Sandbox, SandboxStatus,
SandboxCondition from openshell.datamodel.v1 to openshell.v1. The
MockOpenShell trait impl is still incomplete (new RPC stubs land in a
later task); the lib alone compiles."
```

---

## Task 2: Add `semver` dependency

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/right-openshell/Cargo.toml`

- [ ] **Step 1: Add semver to workspace dependencies**

In `Cargo.toml` at the workspace root, locate `[workspace.dependencies]` and add (alphabetically):

```toml
semver = "1.0"
```

- [ ] **Step 2: Add semver to right-openshell**

In `crates/right-openshell/Cargo.toml`, locate `[dependencies]` and add (alphabetically):

```toml
semver = { workspace = true }
```

- [ ] **Step 3: Verify it builds**

```bash
devenv shell -- cargo build -p right-openshell --lib
```

Expected: success.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock crates/right-openshell/Cargo.toml
git commit -m "build(right-openshell): add semver dependency

Used by openshell version preflight to compare CLI and gateway versions
against MIN_OPENSHELL_VERSION."
```

---

## Task 3: Skeleton `preflight` module — types and a failing test

**Files:**
- Create: `crates/right-openshell/src/preflight.rs`
- Create: `crates/right-openshell/src/preflight_tests.rs`
- Modify: `crates/right-openshell/src/lib.rs`

- [ ] **Step 1: Write the failing test first**

Create `crates/right-openshell/src/preflight_tests.rs`:

```rust
use super::preflight::{MIN_OPENSHELL_VERSION, parse_openshell_cli_version};
use semver::Version;

#[test]
fn min_openshell_version_is_v0_0_50() {
    assert_eq!(MIN_OPENSHELL_VERSION, Version::new(0, 0, 50));
}

#[test]
fn parse_openshell_cli_version_extracts_semver() {
    let v = parse_openshell_cli_version("openshell 0.0.50\n").unwrap();
    assert_eq!(v, Version::new(0, 0, 50));
}

#[test]
fn parse_openshell_cli_version_rejects_garbage() {
    let err = parse_openshell_cli_version("not a version line\n").unwrap_err();
    assert!(
        err.contains("could not parse"),
        "error should mention parse failure, got: {err}"
    );
}

#[test]
fn parse_openshell_cli_version_ignores_trailing_whitespace_and_lines() {
    let v = parse_openshell_cli_version("openshell 0.0.50  \n\nextra line\n").unwrap();
    assert_eq!(v, Version::new(0, 0, 50));
}
```

- [ ] **Step 2: Create the empty module file with the symbol stubs**

Create `crates/right-openshell/src/preflight.rs`:

```rust
//! OpenShell version preflight — verifies the installed CLI binary and
//! the running gateway are both new enough.
//!
//! Wired into bot startup via `crates/bot/src/lib.rs`. Hard-fails the
//! process on mismatch; no quiet degradation.

use semver::Version;
use tonic::transport::Channel;

use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;

/// Minimum supported OpenShell version (both CLI and gateway).
///
/// Below this, the provider gRPC surface is incomplete (no
/// `AttachSandboxProvider` etc.) and Right will not start.
pub const MIN_OPENSHELL_VERSION: Version = Version::new(0, 0, 50);

/// Preflight failure modes. Each variant carries enough context for an
/// actionable diagnostic written to `tracing::error!` before exit.
#[derive(Debug, thiserror::Error)]
pub enum PreflightError {
    #[error("openshell CLI binary not found on PATH (install from https://github.com/NVIDIA/OpenShell)")]
    CliMissing,
    #[error("could not parse `openshell --version` output: {0:?}")]
    CliVersionUnparseable(String),
    #[error(
        "openshell CLI is {found}, need >= {required}; run `brew upgrade openshell` (or your platform equivalent)"
    )]
    CliTooOld { found: Version, required: Version },
    #[error("openshell gateway Health RPC failed: {0}")]
    GatewayUnreachable(#[source] tonic::Status),
    #[error("could not parse openshell gateway version: {0:?}")]
    GatewayVersionUnparseable(String),
    #[error("openshell gateway is {found}, need >= {required}; upgrade your gateway")]
    GatewayTooOld { found: Version, required: Version },
}

/// Parse the output of `openshell --version`, which looks like:
///
/// ```text
/// openshell 0.0.50
/// ```
///
/// Returns the parsed `Version` or a parse-failure string suitable for
/// stuffing into [`PreflightError::CliVersionUnparseable`].
pub fn parse_openshell_cli_version(output: &str) -> Result<Version, String> {
    // Take the first non-empty line; strip a leading "openshell " prefix.
    let first = output
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| format!("could not parse empty output: {output:?}"))?;
    let trimmed = first.trim();
    let rest = trimmed
        .strip_prefix("openshell ")
        .ok_or_else(|| format!("could not parse, expected 'openshell X.Y.Z': {trimmed:?}"))?;
    Version::parse(rest.trim())
        .map_err(|e| format!("could not parse semver from {rest:?}: {e}"))
}

#[cfg(test)]
#[path = "preflight_tests.rs"]
mod tests;

// Note: `openshell_preflight` and the gateway Health probe are added
// in later tasks.
#[allow(dead_code)]
async fn _client_handle_for_grpc_module_export(_: OpenShellClient<Channel>) {}
```

The trailing dummy item keeps the `OpenShellClient` import live until Task 5 wires the real Health probe.

- [ ] **Step 3: Expose the module from `lib.rs`**

Edit `crates/right-openshell/src/lib.rs`. After the existing `pub mod` lines, add:

```rust
pub mod preflight;
```

Make sure ordering is alphabetical with the existing `pub mod openshell;`, `pub mod providers;`, etc.

- [ ] **Step 4: Run the test — verify it passes**

```bash
devenv shell -- cargo test -p right-openshell preflight_tests
```

Expected: 4 passed, 0 failed.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/preflight.rs \
        crates/right-openshell/src/preflight_tests.rs \
        crates/right-openshell/src/lib.rs
git commit -m "feat(right-openshell): scaffold version preflight module

Adds MIN_OPENSHELL_VERSION (=0.0.50), PreflightError variants, and
parse_openshell_cli_version. Wiring into bot startup follows in a later
task."
```

---

## Task 4: `cli_version_check` — invokes `openshell --version` and compares

**Files:**
- Modify: `crates/right-openshell/src/preflight.rs`
- Modify: `crates/right-openshell/src/preflight_tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/right-openshell/src/preflight_tests.rs`:

```rust
use super::preflight::{cli_version_check_str, PreflightError};

#[test]
fn cli_version_check_passes_on_exact_min() {
    let result = cli_version_check_str("openshell 0.0.50\n");
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[test]
fn cli_version_check_passes_on_newer() {
    let result = cli_version_check_str("openshell 0.0.51\n");
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[test]
fn cli_version_check_fails_on_too_old() {
    let result = cli_version_check_str("openshell 0.0.42\n");
    let err = result.unwrap_err();
    let found = match err {
        PreflightError::CliTooOld { found, required } => {
            assert_eq!(required, semver::Version::new(0, 0, 50));
            found
        }
        other => panic!("expected CliTooOld, got: {other:?}"),
    };
    assert_eq!(found, semver::Version::new(0, 0, 42));
}

#[test]
fn cli_version_check_fails_on_unparseable_output() {
    let result = cli_version_check_str("garbage\n");
    assert!(matches!(
        result,
        Err(PreflightError::CliVersionUnparseable(_))
    ));
}
```

- [ ] **Step 2: Run the test — verify it fails**

```bash
devenv shell -- cargo test -p right-openshell preflight_tests::cli_version_check
```

Expected: 4 test failures with `error[E0432]: unresolved import` or `cannot find function 'cli_version_check_str' in module`.

- [ ] **Step 3: Implement `cli_version_check_str`**

Append to `crates/right-openshell/src/preflight.rs` (above the `#[cfg(test)]` line):

```rust
/// Pure helper used by [`cli_version_check`] and tests. Takes the raw
/// `openshell --version` stdout and returns Ok on `>= MIN_OPENSHELL_VERSION`,
/// `PreflightError::CliTooOld` if older, `CliVersionUnparseable` on garbage.
pub fn cli_version_check_str(output: &str) -> Result<(), PreflightError> {
    let found = parse_openshell_cli_version(output)
        .map_err(PreflightError::CliVersionUnparseable)?;
    if found < MIN_OPENSHELL_VERSION {
        return Err(PreflightError::CliTooOld {
            found,
            required: MIN_OPENSHELL_VERSION,
        });
    }
    Ok(())
}

/// Spawn `openshell --version` and check against [`MIN_OPENSHELL_VERSION`].
/// Returns `CliMissing` if the binary isn't on PATH.
pub async fn cli_version_check() -> Result<(), PreflightError> {
    let out = tokio::process::Command::new("openshell")
        .arg("--version")
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PreflightError::CliMissing
            } else {
                PreflightError::CliVersionUnparseable(format!("spawn failed: {e:#}"))
            }
        })?;
    if !out.status.success() {
        return Err(PreflightError::CliVersionUnparseable(format!(
            "openshell --version exited {} stderr={:?}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr),
        )));
    }
    cli_version_check_str(&String::from_utf8_lossy(&out.stdout))
}
```

- [ ] **Step 4: Run the tests — verify they pass**

```bash
devenv shell -- cargo test -p right-openshell preflight_tests
```

Expected: 8 passed, 0 failed.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/preflight.rs \
        crates/right-openshell/src/preflight_tests.rs
git commit -m "feat(right-openshell): cli_version_check spawns openshell --version

Adds cli_version_check_str (pure, tested) and cli_version_check (spawns
the binary). Returns PreflightError::CliMissing if the binary is absent,
CliTooOld if below MIN_OPENSHELL_VERSION, CliVersionUnparseable on
garbage output."
```

---

## Task 5: `gateway_version_check` — Health RPC + mock server test

**Files:**
- Modify: `crates/right-openshell/src/preflight.rs`
- Modify: `crates/right-openshell/src/preflight_tests.rs`
- Modify: `crates/right-openshell/src/openshell_tests.rs` (need to expose `mock_client` + `start_mock_server` outside the test module so preflight can reuse them)

- [ ] **Step 1: Promote `start_mock_server` and `mock_client` to a shared test-helper module**

In `crates/right-openshell/src/openshell_tests.rs`, both `start_mock_server` (line ~905) and `mock_client` (line ~929) are currently private to that test module. Move them and the `MockOpenShell` struct into a new submodule visible from sibling test files.

Edit the top of `crates/right-openshell/src/openshell_tests.rs` and locate the `start_mock_server` and `mock_client` definitions. Above them, add:

```rust
// The mock server + plain-HTTP client below are shared with sibling
// test modules (e.g. `preflight_tests`, `providers_tests`).
pub(crate) use mock::{MockOpenShell, mock_client, start_mock_server};
```

Then wrap the existing definitions in `mod mock { ... }`. The simpler refactor — and the one this task uses — is to extract them into a new file `crates/right-openshell/src/test_mock_server.rs` and import from it.

Create `crates/right-openshell/src/test_mock_server.rs`:

```rust
//! Shared in-process mock OpenShell gRPC server, used by sibling test
//! modules (`openshell_tests`, `preflight_tests`, `providers_tests`).
//! Plain-HTTP (no TLS) — tests connect via `mock_client(addr)` to skip
//! the mTLS setup required by production `connect_grpc`.

#![cfg(test)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Channel, Server};

use crate::openshell_proto::openshell as proto;
use proto::v1::open_shell_client::OpenShellClient;
use proto::v1::open_shell_server::{OpenShell, OpenShellServer};

// Type aliases for streaming RPCs the mock doesn't use.
type EmptyExecStream =
    tokio_stream::wrappers::ReceiverStream<Result<proto::v1::ExecSandboxEvent, tonic::Status>>;
type EmptyWatchStream =
    tokio_stream::wrappers::ReceiverStream<Result<proto::v1::SandboxStreamEvent, tonic::Status>>;

/// A configurable mock `OpenShell` service. Each field that begins with
/// `override_` lets a test inject a closure returning the response (or
/// `tonic::Status` error) for that RPC. All un-overridden RPCs return
/// `Unimplemented`.
///
/// Sandbox-RPC defaults (`get_sandbox_phase`, `get_sandbox_status`) are
/// preserved from the previous `MockOpenShell` for compatibility with
/// existing tests in `openshell_tests.rs`.
#[derive(Default)]
pub struct MockOpenShell {
    pub get_sandbox_phase: Arc<AtomicI32>,
    pub get_sandbox_status: Option<proto::v1::SandboxStatus>,

    // Provider mocks — set per-test. `None` = return Unimplemented.
    pub mock_health: Option<Box<dyn Fn() -> Result<proto::v1::HealthResponse, tonic::Status> + Send + Sync>>,
    pub mock_create_provider: Option<Box<dyn Fn(proto::v1::CreateProviderRequest) -> Result<proto::v1::ProviderResponse, tonic::Status> + Send + Sync>>,
    pub mock_get_provider: Option<Box<dyn Fn(proto::v1::GetProviderRequest) -> Result<proto::v1::ProviderResponse, tonic::Status> + Send + Sync>>,
    pub mock_update_provider: Option<Box<dyn Fn(proto::v1::UpdateProviderRequest) -> Result<proto::v1::ProviderResponse, tonic::Status> + Send + Sync>>,
    pub mock_delete_provider: Option<Box<dyn Fn(proto::v1::DeleteProviderRequest) -> Result<proto::v1::DeleteProviderResponse, tonic::Status> + Send + Sync>>,
    pub mock_list_providers: Option<Box<dyn Fn(proto::v1::ListProvidersRequest) -> Result<proto::v1::ListProvidersResponse, tonic::Status> + Send + Sync>>,
    pub mock_attach_sandbox_provider: Option<Box<dyn Fn(proto::v1::AttachSandboxProviderRequest) -> Result<proto::v1::AttachSandboxProviderResponse, tonic::Status> + Send + Sync>>,
    pub mock_detach_sandbox_provider: Option<Box<dyn Fn(proto::v1::DetachSandboxProviderRequest) -> Result<proto::v1::DetachSandboxProviderResponse, tonic::Status> + Send + Sync>>,
    pub mock_list_sandbox_providers: Option<Box<dyn Fn(proto::v1::ListSandboxProvidersRequest) -> Result<proto::v1::ListSandboxProvidersResponse, tonic::Status> + Send + Sync>>,
    pub mock_get_sandbox_provider_environment: Option<Box<dyn Fn(proto::v1::GetSandboxProviderEnvironmentRequest) -> Result<proto::v1::GetSandboxProviderEnvironmentResponse, tonic::Status> + Send + Sync>>,
}

impl MockOpenShell {
    pub fn not_found() -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(-1)),
            ..Default::default()
        }
    }
    pub fn with_phase(phase: i32) -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(phase)),
            ..Default::default()
        }
    }
    pub fn with_phase_and_status(phase: i32, status: proto::v1::SandboxStatus) -> Self {
        Self {
            get_sandbox_phase: Arc::new(AtomicI32::new(phase)),
            get_sandbox_status: Some(status),
            ..Default::default()
        }
    }
    pub fn with_shared_phase(phase: Arc<AtomicI32>) -> Self {
        Self {
            get_sandbox_phase: phase,
            ..Default::default()
        }
    }
}

#[tonic::async_trait]
impl OpenShell for MockOpenShell {
    async fn get_sandbox(
        &self,
        _: tonic::Request<proto::v1::GetSandboxRequest>,
    ) -> Result<tonic::Response<proto::v1::SandboxResponse>, tonic::Status> {
        let phase = self.get_sandbox_phase.load(Ordering::Relaxed);
        if phase < 0 {
            return Err(tonic::Status::not_found("sandbox not found"));
        }
        Ok(tonic::Response::new(proto::v1::SandboxResponse {
            sandbox: Some(proto::datamodel::v1::Sandbox {
                metadata: Some(proto::datamodel::v1::ObjectMeta {
                    id: "mock-sandbox-id".into(),
                    name: "mock-sandbox".into(),
                    ..Default::default()
                }),
                phase,
                status: self.get_sandbox_status.clone(),
                ..Default::default()
            }),
        }))
    }

    async fn health(
        &self,
        _: tonic::Request<proto::v1::HealthRequest>,
    ) -> Result<tonic::Response<proto::v1::HealthResponse>, tonic::Status> {
        match &self.mock_health {
            Some(f) => f().map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    async fn create_provider(
        &self,
        req: tonic::Request<proto::v1::CreateProviderRequest>,
    ) -> Result<tonic::Response<proto::v1::ProviderResponse>, tonic::Status> {
        match &self.mock_create_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
    async fn get_provider(
        &self,
        req: tonic::Request<proto::v1::GetProviderRequest>,
    ) -> Result<tonic::Response<proto::v1::ProviderResponse>, tonic::Status> {
        match &self.mock_get_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
    async fn update_provider(
        &self,
        req: tonic::Request<proto::v1::UpdateProviderRequest>,
    ) -> Result<tonic::Response<proto::v1::ProviderResponse>, tonic::Status> {
        match &self.mock_update_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
    async fn delete_provider(
        &self,
        req: tonic::Request<proto::v1::DeleteProviderRequest>,
    ) -> Result<tonic::Response<proto::v1::DeleteProviderResponse>, tonic::Status> {
        match &self.mock_delete_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
    async fn list_providers(
        &self,
        req: tonic::Request<proto::v1::ListProvidersRequest>,
    ) -> Result<tonic::Response<proto::v1::ListProvidersResponse>, tonic::Status> {
        match &self.mock_list_providers {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
    async fn attach_sandbox_provider(
        &self,
        req: tonic::Request<proto::v1::AttachSandboxProviderRequest>,
    ) -> Result<tonic::Response<proto::v1::AttachSandboxProviderResponse>, tonic::Status> {
        match &self.mock_attach_sandbox_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
    async fn detach_sandbox_provider(
        &self,
        req: tonic::Request<proto::v1::DetachSandboxProviderRequest>,
    ) -> Result<tonic::Response<proto::v1::DetachSandboxProviderResponse>, tonic::Status> {
        match &self.mock_detach_sandbox_provider {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
    async fn list_sandbox_providers(
        &self,
        req: tonic::Request<proto::v1::ListSandboxProvidersRequest>,
    ) -> Result<tonic::Response<proto::v1::ListSandboxProvidersResponse>, tonic::Status> {
        match &self.mock_list_sandbox_providers {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }
    async fn get_sandbox_provider_environment(
        &self,
        req: tonic::Request<proto::v1::GetSandboxProviderEnvironmentRequest>,
    ) -> Result<tonic::Response<proto::v1::GetSandboxProviderEnvironmentResponse>, tonic::Status> {
        match &self.mock_get_sandbox_provider_environment {
            Some(f) => f(req.into_inner()).map(tonic::Response::new),
            None => Err(tonic::Status::unimplemented("stub")),
        }
    }

    // All other RPCs return Unimplemented stubs. Copy the existing stub
    // block from openshell_tests.rs (lines ~700-902 in the current file)
    // for the v0.0.42 RPCs: create_sandbox, list_sandboxes, delete_sandbox,
    // create_ssh_session, revoke_ssh_session, exec_sandbox, get_sandbox_config,
    // get_gateway_config, update_config, get_sandbox_policy_status,
    // list_sandbox_policies, report_policy_status, get_sandbox_logs,
    // push_sandbox_logs, watch_sandbox, submit_policy_analysis,
    // get_draft_policy, approve_draft_chunk, reject_draft_chunk,
    // approve_all_draft_chunks, edit_draft_chunk, undo_draft_chunk,
    // clear_draft_chunks, get_draft_history.
    //
    // ADD new v0.0.50 method stubs (each returns Err(tonic::Status::unimplemented("stub"))):
    //   - expose_service
    //   - get_service
    //   - list_services
    //   - delete_service
    //   - forward_tcp                 (streaming; type ForwardTcpStream = ...)
    //   - exec_sandbox_interactive    (streaming; type ExecSandboxInteractiveStream = ...)
    //   - list_provider_profiles
    //   - get_provider_profile
    //   - import_provider_profiles
    //   - lint_provider_profiles
    //   - delete_provider_profile
    //   - get_provider_refresh_status
    //   - configure_provider_refresh
    //   - rotate_provider_credential
    //   - delete_provider_refresh
    //
    // For each, copy the shape from the existing exec_sandbox stub (for streaming
    // RPCs) or the existing create_sandbox stub (for unary). The exact request/
    // response types live under `crate::openshell_proto::openshell::v1`; let the
    // compiler tell you the names by attempting to build with stubs missing.
}

pub async fn start_mock_server(mock: MockOpenShell) -> (SocketAddr, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        Server::builder()
            .add_service(OpenShellServer::new(mock))
            .serve_with_incoming_shutdown(
                TcpListenerStream::new(listener),
                async {
                    let _ = rx.await;
                },
            )
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, tx)
}

/// Connect a plain (non-TLS) `OpenShellClient` to the mock at `addr`.
pub async fn mock_client(addr: SocketAddr) -> OpenShellClient<Channel> {
    let channel = Channel::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    OpenShellClient::new(channel)
}
```

In `crates/right-openshell/src/lib.rs`, add:

```rust
#[cfg(test)]
mod test_mock_server;
```

In `crates/right-openshell/src/openshell_tests.rs`:

1. Delete the existing `MockOpenShell` struct definition (lines ~631-665), its `impl MockOpenShell` block, its `#[tonic::async_trait] impl open_shell_server::OpenShell for MockOpenShell` block, and the `start_mock_server` / `mock_client` functions.
2. At the top, add:

```rust
use crate::test_mock_server::{MockOpenShell, mock_client, start_mock_server};
```

- [ ] **Step 2: Confirm existing sandbox tests still pass after the refactor**

```bash
devenv shell -- cargo test -p right-openshell openshell_tests::is_sandbox_ready
```

Expected: existing tests using `MockOpenShell::not_found()`, `with_phase(_)`, etc. still pass via the re-exports.

- [ ] **Step 3: Write the failing gateway version preflight test**

Append to `crates/right-openshell/src/preflight_tests.rs`:

```rust
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::test_mock_server::{MockOpenShell, mock_client, start_mock_server};

#[tokio::test]
async fn gateway_version_check_passes_on_exact_min() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.50".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let result = super::preflight::gateway_version_check(&mut client).await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[tokio::test]
async fn gateway_version_check_fails_on_too_old() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.49".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let err = super::preflight::gateway_version_check(&mut client)
        .await
        .unwrap_err();
    match err {
        PreflightError::GatewayTooOld { found, required } => {
            assert_eq!(found, semver::Version::new(0, 0, 49));
            assert_eq!(required, semver::Version::new(0, 0, 50));
        }
        other => panic!("expected GatewayTooOld, got: {other:?}"),
    }
}

#[tokio::test]
async fn gateway_version_check_fails_on_unparseable_version() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "garbage".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let result = super::preflight::gateway_version_check(&mut client).await;
    assert!(matches!(
        result,
        Err(PreflightError::GatewayVersionUnparseable(_))
    ));
}

#[tokio::test]
async fn gateway_version_check_fails_when_health_rpc_errors() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| Err(tonic::Status::internal("boom")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let result = super::preflight::gateway_version_check(&mut client).await;
    assert!(matches!(result, Err(PreflightError::GatewayUnreachable(_))));
}
```

- [ ] **Step 4: Run the tests — verify failure**

```bash
devenv shell -- cargo test -p right-openshell preflight_tests::gateway_version_check
```

Expected: 4 tests fail with `cannot find function 'gateway_version_check'`.

- [ ] **Step 5: Implement `gateway_version_check`**

In `crates/right-openshell/src/preflight.rs`, replace the `_client_handle_for_grpc_module_export` placeholder with:

```rust
use crate::openshell_proto::openshell::v1::HealthRequest;

/// Issue a `Health` RPC and verify the returned version is
/// `>= MIN_OPENSHELL_VERSION`.
pub async fn gateway_version_check(
    client: &mut OpenShellClient<Channel>,
) -> Result<(), PreflightError> {
    let resp = client
        .health(HealthRequest {})
        .await
        .map_err(PreflightError::GatewayUnreachable)?
        .into_inner();
    let found = Version::parse(resp.version.trim())
        .map_err(|e| PreflightError::GatewayVersionUnparseable(format!("{}: {e}", resp.version)))?;
    if found < MIN_OPENSHELL_VERSION {
        return Err(PreflightError::GatewayTooOld {
            found,
            required: MIN_OPENSHELL_VERSION,
        });
    }
    Ok(())
}
```

- [ ] **Step 6: Run the tests — verify they pass**

```bash
devenv shell -- cargo test -p right-openshell preflight_tests
```

Expected: 12 passed, 0 failed.

- [ ] **Step 7: Commit**

```bash
git add crates/right-openshell/src/test_mock_server.rs \
        crates/right-openshell/src/openshell_tests.rs \
        crates/right-openshell/src/lib.rs \
        crates/right-openshell/src/preflight.rs \
        crates/right-openshell/src/preflight_tests.rs
git commit -m "feat(right-openshell): gateway_version_check via Health RPC

Promotes MockOpenShell, start_mock_server, mock_client into a shared
crate::test_mock_server module (re-used by providers_tests in later
tasks). Adds per-RPC override closures so tests configure exactly the
methods they exercise. Implements gateway_version_check against the
generated Health RPC."
```

---

## Task 6: `openshell_preflight` — top-level entry point

**Files:**
- Modify: `crates/right-openshell/src/preflight.rs`
- Modify: `crates/right-openshell/src/preflight_tests.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/right-openshell/src/preflight_tests.rs`:

```rust
// Integration-style: spawn the mock server, hand a configured client to
// openshell_preflight, and assert it composes cli + gateway checks.
//
// The CLI half is exercised by spawning a fake `openshell --version`
// shim via env override. To keep this test hermetic without setting
// process env (forbidden by AGENTS.rust.md), we route via
// `openshell_preflight_with` which takes both a closure returning the
// CLI version string and the gRPC client.

use super::preflight::openshell_preflight_with;

#[tokio::test]
async fn openshell_preflight_with_succeeds_when_both_ok() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.50".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let result = openshell_preflight_with(
        || async { Ok("openshell 0.0.50\n".to_string()) },
        &mut client,
    )
    .await;
    assert!(result.is_ok(), "expected Ok, got: {result:?}");
}

#[tokio::test]
async fn openshell_preflight_with_fails_fast_on_cli_too_old() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            // Even though gateway is good, CLI check runs first and fails.
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.50".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let result = openshell_preflight_with(
        || async { Ok("openshell 0.0.42\n".to_string()) },
        &mut client,
    )
    .await;
    assert!(matches!(result, Err(PreflightError::CliTooOld { .. })));
}

#[tokio::test]
async fn openshell_preflight_with_fails_on_gateway_too_old() {
    let mock = MockOpenShell {
        mock_health: Some(Box::new(|| {
            Ok(proto_v1::HealthResponse {
                status: 0,
                version: "0.0.49".into(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let result = openshell_preflight_with(
        || async { Ok("openshell 0.0.50\n".to_string()) },
        &mut client,
    )
    .await;
    assert!(matches!(result, Err(PreflightError::GatewayTooOld { .. })));
}
```

- [ ] **Step 2: Run the test — verify failure**

```bash
devenv shell -- cargo test -p right-openshell preflight_tests::openshell_preflight_with
```

Expected: 3 tests fail with `cannot find function openshell_preflight_with`.

- [ ] **Step 3: Implement the entry point**

Append to `crates/right-openshell/src/preflight.rs`:

```rust
use std::future::Future;

/// Top-level preflight. CLI check first (fast-fail when binary is
/// missing or too old), then gateway Health.
///
/// Production wrapper around [`openshell_preflight_with`]. Spawns
/// `openshell --version` and connects to the gRPC gateway.
pub async fn openshell_preflight(
    client: &mut OpenShellClient<Channel>,
) -> Result<(), PreflightError> {
    openshell_preflight_with(
        || async {
            let out = tokio::process::Command::new("openshell")
                .arg("--version")
                .output()
                .await
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        PreflightError::CliMissing
                    } else {
                        PreflightError::CliVersionUnparseable(format!("spawn failed: {e:#}"))
                    }
                })?;
            if !out.status.success() {
                return Err(PreflightError::CliVersionUnparseable(format!(
                    "openshell --version exited {}",
                    out.status.code().unwrap_or(-1)
                )));
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        },
        client,
    )
    .await
}

/// Test-friendly form of [`openshell_preflight`]. Takes an
/// async closure that returns the raw `openshell --version` stdout,
/// so tests can inject a fake without `std::env::set_var`.
pub async fn openshell_preflight_with<F, Fut>(
    cli_version_source: F,
    client: &mut OpenShellClient<Channel>,
) -> Result<(), PreflightError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String, PreflightError>>,
{
    let cli_output = cli_version_source().await?;
    cli_version_check_str(&cli_output)?;
    gateway_version_check(client).await?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests — verify they pass**

```bash
devenv shell -- cargo test -p right-openshell preflight_tests
```

Expected: 15 passed, 0 failed.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/preflight.rs \
        crates/right-openshell/src/preflight_tests.rs
git commit -m "feat(right-openshell): openshell_preflight composes cli + gateway

Adds the top-level openshell_preflight (production) and
openshell_preflight_with (test-friendly form taking an injected CLI
version source). CLI check runs first for fast-fail."
```

---

## Task 7: Migrate provider CRUD to gRPC

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`
- Create: `crates/right-openshell/src/providers_tests.rs`

This task replaces the **public function signatures and bodies** of `create_provider`, `get_provider`, `update_provider`, and `delete_provider` in one commit. The new signatures take `&mut OpenShellClient<Channel>` (no `&GatewayEndpoint`). Their callers in `internal_api_providers.rs` are updated in Task 9.

The other provider functions (`list_providers_by_prefix`, `attach_to_sandbox`, `detach_from_sandbox`, `list_attached`, `get_sandbox_provider_environment`) are migrated in Task 8.

- [ ] **Step 1: Write the failing tests**

Create `crates/right-openshell/src/providers_tests.rs`:

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::openshell_proto::openshell::datamodel::v1 as datamodel;
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::providers::{
    ProviderError, ProviderSpec, create_provider, delete_provider, get_provider, update_provider,
};
use crate::test_mock_server::{MockOpenShell, mock_client, start_mock_server};

#[tokio::test]
async fn create_provider_sends_typed_request() {
    let seen: Arc<Mutex<Option<proto_v1::CreateProviderRequest>>> = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let mock = MockOpenShell {
        mock_create_provider: Some(Box::new(move |req| {
            *seen_clone.lock().unwrap() = Some(req.clone());
            Ok(proto_v1::ProviderResponse {
                provider: req.provider,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let mut creds = HashMap::new();
    creds.insert("MY_TOKEN".to_string(), "secret-value".to_string());
    let spec = ProviderSpec {
        name: "test-prov".into(),
        type_: "generic".into(),
        credentials: creds,
        config: HashMap::new(),
    };
    let created = create_provider(&mut client, &spec).await.unwrap();
    assert_eq!(created.name, "test-prov");

    let req = seen.lock().unwrap().clone().unwrap();
    let p = req.provider.unwrap();
    assert_eq!(p.metadata.unwrap().name, "test-prov");
    assert_eq!(p.r#type, "generic");
    assert_eq!(p.credentials.get("MY_TOKEN"), Some(&"secret-value".to_string()));
}

#[tokio::test]
async fn get_provider_maps_not_found() {
    let mock = MockOpenShell {
        mock_get_provider: Some(Box::new(|_| Err(tonic::Status::not_found("missing")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let err = get_provider(&mut client, "missing").await.unwrap_err();
    match err {
        ProviderError::NotFound(name) => assert_eq!(name, "missing"),
        other => panic!("expected NotFound, got: {other:?}"),
    }
}

#[tokio::test]
async fn get_provider_maps_other_status_to_grpc() {
    let mock = MockOpenShell {
        mock_get_provider: Some(Box::new(|_| Err(tonic::Status::internal("boom")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let err = get_provider(&mut client, "x").await.unwrap_err();
    match err {
        ProviderError::Grpc(msg) => {
            assert!(msg.contains("Internal"), "{msg}");
            assert!(msg.contains("boom"), "{msg}");
        }
        other => panic!("expected Grpc, got: {other:?}"),
    }
}

#[tokio::test]
async fn get_provider_decodes_provider_payload() {
    let mock = MockOpenShell {
        mock_get_provider: Some(Box::new(|_| {
            Ok(proto_v1::ProviderResponse {
                provider: Some(datamodel::Provider {
                    metadata: Some(datamodel::ObjectMeta {
                        name: "p1".into(),
                        ..Default::default()
                    }),
                    r#type: "generic".into(),
                    config: HashMap::new(),
                    credentials: HashMap::new(),
                    credential_expires_at_ms: HashMap::new(),
                }),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let p = get_provider(&mut client, "p1").await.unwrap();
    assert_eq!(p.name, "p1");
    assert_eq!(p.type_, "generic");
}

#[tokio::test]
async fn update_provider_round_trip() {
    let seen: Arc<Mutex<Option<proto_v1::UpdateProviderRequest>>> = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let mock = MockOpenShell {
        mock_update_provider: Some(Box::new(move |req| {
            *seen_clone.lock().unwrap() = Some(req.clone());
            Ok(proto_v1::ProviderResponse {
                provider: req.provider,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;

    let mut creds = HashMap::new();
    creds.insert("ROT".into(), "v2".into());
    let spec = ProviderSpec {
        name: "rot".into(),
        type_: "generic".into(),
        credentials: creds,
        config: HashMap::new(),
    };
    let updated = update_provider(&mut client, &spec).await.unwrap();
    assert_eq!(updated.name, "rot");
    let req = seen.lock().unwrap().clone().unwrap();
    let p = req.provider.unwrap();
    assert_eq!(p.credentials.get("ROT"), Some(&"v2".to_string()));
}

#[tokio::test]
async fn delete_provider_returns_ok_on_success() {
    let mock = MockOpenShell {
        mock_delete_provider: Some(Box::new(|_| {
            Ok(proto_v1::DeleteProviderResponse { deleted: true })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    delete_provider(&mut client, "x").await.unwrap();
}

#[tokio::test]
async fn delete_provider_maps_not_found() {
    let mock = MockOpenShell {
        mock_delete_provider: Some(Box::new(|_| Err(tonic::Status::not_found("absent")))),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let err = delete_provider(&mut client, "x").await.unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)));
}

#[tokio::test]
async fn create_provider_request_debug_does_not_leak_credentials() {
    let mut creds = HashMap::new();
    creds.insert("MY_TOKEN".into(), "super-secret-value-xyz".into());
    let spec = ProviderSpec {
        name: "x".into(),
        type_: "generic".into(),
        credentials: creds,
        config: HashMap::new(),
    };
    let debug = format!("{spec:?}");
    assert!(
        !debug.contains("super-secret-value-xyz"),
        "credential value leaked through ProviderSpec Debug impl: {debug}"
    );
    assert!(debug.contains("redacted"), "debug should mention redaction");
}
```

In `crates/right-openshell/src/providers.rs`, replace the `#[cfg(test)]` test module attachment block (if absent today, locate the end of the file and add):

```rust
#[cfg(test)]
#[path = "providers_tests.rs"]
mod tests;
```

- [ ] **Step 2: Run the test — verify it fails to compile**

```bash
devenv shell -- cargo test -p right-openshell providers_tests
```

Expected: compile errors — `create_provider`/`get_provider`/`update_provider`/`delete_provider` still take `&GatewayEndpoint`, not `&mut OpenShellClient<Channel>`.

- [ ] **Step 3: Rewrite `create_provider` / `get_provider` / `update_provider` / `delete_provider`**

In `crates/right-openshell/src/providers.rs`:

1. **Remove** the `use tokio::process::Command;` and `use std::process::Stdio;` imports.
2. **Add** new imports near the top (preserving the existing ones):

```rust
use tonic::transport::Channel;

use crate::openshell_proto::openshell::datamodel::v1 as datamodel;
use crate::openshell_proto::openshell::v1 as proto_v1;
use crate::openshell_proto::openshell::v1::open_shell_client::OpenShellClient;
```

3. **Add** the conversion helpers (place above `pub fn profile_catalog()`):

```rust
/// Map a `tonic::Status` to a `ProviderError`, preserving NotFound
/// semantics. `name_for_not_found` is the resource identifier used in
/// the `ProviderError::NotFound(name)` variant.
fn classify_status(status: tonic::Status, name_for_not_found: &str) -> ProviderError {
    if status.code() == tonic::Code::NotFound {
        ProviderError::NotFound(name_for_not_found.to_string())
    } else {
        ProviderError::Grpc(format!("{}: {}", status.code(), status.message()))
    }
}

/// Convert a wire-level [`datamodel::Provider`] into the host-facing
/// [`Provider`] struct. Credentials and `credential_expires_at_ms` are
/// intentionally dropped — Right never persists them on host.
fn provider_from_proto(p: datamodel::Provider) -> Provider {
    let metadata = p.metadata.unwrap_or_default();
    let updated_at = parse_object_meta_updated_at(&metadata);
    Provider {
        name: metadata.name,
        type_: p.r#type,
        config: p.config,
        updated_at,
    }
}

/// Derive a `DateTime<Utc>` from `ObjectMeta.created_at_ms` (int64
/// milliseconds since Unix epoch). v0.0.50 `ObjectMeta` does NOT have
/// an `updated_at` field — there is no last-modified timestamp on the
/// gateway, only creation. `Provider.updated_at` therefore holds the
/// creation time; callers that want a "modified at" must look at
/// `metadata.resource_version` instead, which is a monotonic counter
/// bumped on each update.
fn parse_object_meta_updated_at(meta: &datamodel::ObjectMeta) -> Option<chrono::DateTime<chrono::Utc>> {
    if meta.created_at_ms <= 0 {
        return None;
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(meta.created_at_ms)
}

/// Build a `datamodel::Provider` payload from a host-facing `ProviderSpec`.
fn proto_provider_from_spec(spec: &ProviderSpec) -> datamodel::Provider {
    datamodel::Provider {
        metadata: Some(datamodel::ObjectMeta {
            name: spec.name.clone(),
            ..Default::default()
        }),
        r#type: spec.type_.clone(),
        credentials: spec.credentials.clone(),
        config: spec.config.clone(),
        credential_expires_at_ms: HashMap::new(),
    }
}
```

4. **Replace** `create_provider`, `get_provider`, `update_provider`, `delete_provider` bodies with the gRPC versions:

```rust
pub async fn create_provider(
    client: &mut OpenShellClient<Channel>,
    spec: &ProviderSpec,
) -> Result<Provider, ProviderError> {
    let req = proto_v1::CreateProviderRequest {
        provider: Some(proto_provider_from_spec(spec)),
    };
    let resp = client
        .create_provider(req)
        .await
        .map_err(|s| classify_status(s, &spec.name))?
        .into_inner();
    let p = resp
        .provider
        .ok_or_else(|| ProviderError::Grpc("CreateProvider: missing provider in response".into()))?;
    Ok(provider_from_proto(p))
}

pub async fn get_provider(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> Result<Provider, ProviderError> {
    let req = proto_v1::GetProviderRequest {
        name: name.to_string(),
    };
    let resp = client
        .get_provider(req)
        .await
        .map_err(|s| classify_status(s, name))?
        .into_inner();
    let p = resp
        .provider
        .ok_or_else(|| ProviderError::Grpc("GetProvider: missing provider in response".into()))?;
    Ok(provider_from_proto(p))
}

pub async fn update_provider(
    client: &mut OpenShellClient<Channel>,
    spec: &ProviderSpec,
) -> Result<Provider, ProviderError> {
    let req = proto_v1::UpdateProviderRequest {
        provider: Some(proto_provider_from_spec(spec)),
        credential_expires_at_ms: HashMap::new(),
    };
    let resp = client
        .update_provider(req)
        .await
        .map_err(|s| classify_status(s, &spec.name))?
        .into_inner();
    let p = resp
        .provider
        .ok_or_else(|| ProviderError::Grpc("UpdateProvider: missing provider in response".into()))?;
    Ok(provider_from_proto(p))
}

pub async fn delete_provider(
    client: &mut OpenShellClient<Channel>,
    name: &str,
) -> Result<(), ProviderError> {
    let req = proto_v1::DeleteProviderRequest {
        name: name.to_string(),
    };
    client
        .delete_provider(req)
        .await
        .map_err(|s| classify_status(s, name))?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests — verify they pass**

```bash
devenv shell -- cargo test -p right-openshell providers_tests
```

Expected: 8 passed, 0 failed.

If `cargo test` fails to compile because of unrelated functions still referencing CLI helpers (`run_cli`, `parse_provider_json`, etc.), DO NOT fix them yet — Task 8 migrates the rest. Use a narrower test command:

```bash
devenv shell -- cargo test -p right-openshell --lib providers_tests --no-fail-fast
```

If that still fails because the *lib* doesn't compile (other CLI-bound provider functions are still in the file using the now-removed imports), restore `use std::process::Stdio;` and `use tokio::process::Command;` to keep them building, plus keep the unused-import `#[allow(unused_imports)]` until Task 8.

- [ ] **Step 5: Commit**

```bash
git add crates/right-openshell/src/providers.rs \
        crates/right-openshell/src/providers_tests.rs
git commit -m "refactor(providers): migrate CRUD to gRPC

create_provider / get_provider / update_provider / delete_provider now
take &mut OpenShellClient<Channel> and call CreateProvider /
GetProvider / UpdateProvider / DeleteProvider RPCs from
openshell.v1.OpenShell. tonic::Code::NotFound maps to
ProviderError::NotFound. Mock-server tests cover request payload,
response decode, and NotFound classification.

Caller updates in internal_api_providers.rs land in a later task; this
commit may leave the lib temporarily uncompilable for non-CRUD provider
functions until task 8."
```

---

## Task 8: Migrate `list_providers_by_prefix`, `attach_to_sandbox`, `detach_from_sandbox`, `list_attached`, `get_sandbox_provider_environment` to gRPC

**Files:**
- Modify: `crates/right-openshell/src/providers.rs`
- Modify: `crates/right-openshell/src/providers_tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/right-openshell/src/providers_tests.rs`:

```rust
use crate::providers::{
    attach_to_sandbox, detach_from_sandbox, get_sandbox_provider_environment, list_attached,
    list_providers_by_prefix,
};

#[tokio::test]
async fn list_providers_by_prefix_filters_client_side() {
    let mock = MockOpenShell {
        mock_list_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListProvidersResponse {
                providers: vec![
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "agent1-acme".into(),
                            ..Default::default()
                        }),
                        r#type: "generic".into(),
                        ..Default::default()
                    },
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "agent2-acme".into(),
                            ..Default::default()
                        }),
                        r#type: "generic".into(),
                        ..Default::default()
                    },
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "agent1-other".into(),
                            ..Default::default()
                        }),
                        r#type: "generic".into(),
                        ..Default::default()
                    },
                ],
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let mut got = list_providers_by_prefix(&mut client, "agent1-")
        .await
        .unwrap();
    got.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].name, "agent1-acme");
    assert_eq!(got[1].name, "agent1-other");
}

#[tokio::test]
async fn attach_to_sandbox_sends_typed_request() {
    let seen: Arc<Mutex<Option<proto_v1::AttachSandboxProviderRequest>>> =
        Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let mock = MockOpenShell {
        mock_attach_sandbox_provider: Some(Box::new(move |req| {
            *seen_clone.lock().unwrap() = Some(req.clone());
            Ok(proto_v1::AttachSandboxProviderResponse {
                sandbox: None,
                attached: true,
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    attach_to_sandbox(&mut client, "sbox1", "prov1").await.unwrap();
    let req = seen.lock().unwrap().clone().unwrap();
    assert_eq!(req.sandbox_name, "sbox1");
    assert_eq!(req.provider_name, "prov1");
}

#[tokio::test]
async fn detach_from_sandbox_not_found() {
    let mock = MockOpenShell {
        mock_detach_sandbox_provider: Some(Box::new(|_| {
            Err(tonic::Status::not_found("not attached"))
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let err = detach_from_sandbox(&mut client, "sbox", "prov")
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::NotFound(_)));
}

#[tokio::test]
async fn list_attached_returns_names() {
    let mock = MockOpenShell {
        mock_list_sandbox_providers: Some(Box::new(|_| {
            Ok(proto_v1::ListSandboxProvidersResponse {
                providers: vec![
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "a".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    datamodel::Provider {
                        metadata: Some(datamodel::ObjectMeta {
                            name: "b".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let mut names = list_attached(&mut client, "sbox1").await.unwrap();
    names.sort();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
}

#[tokio::test]
async fn get_sandbox_provider_environment_returns_map() {
    let mock = MockOpenShell {
        mock_get_sandbox_provider_environment: Some(Box::new(|req| {
            assert_eq!(req.sandbox_id, "sbox-id-xyz");
            let mut env = HashMap::new();
            env.insert("MY_TOKEN".into(), "openshell:resolve:env:v1_MY_TOKEN".into());
            Ok(proto_v1::GetSandboxProviderEnvironmentResponse {
                environment: env,
                provider_env_revision: 1,
                credential_expires_at_ms: HashMap::new(),
            })
        })),
        ..Default::default()
    };
    let (addr, _shutdown) = start_mock_server(mock).await;
    let mut client = mock_client(addr).await;
    let env = get_sandbox_provider_environment(&mut client, "sbox-id-xyz")
        .await
        .unwrap();
    assert_eq!(
        env.get("MY_TOKEN"),
        Some(&"openshell:resolve:env:v1_MY_TOKEN".to_string())
    );
}
```

- [ ] **Step 2: Run the test — verify it fails**

```bash
devenv shell -- cargo test -p right-openshell providers_tests
```

Expected: compile errors — the migration targets still have CLI signatures.

- [ ] **Step 3: Implement the remaining gRPC versions**

In `crates/right-openshell/src/providers.rs`, replace the bodies of `list_providers_by_prefix`, `attach_to_sandbox`, `detach_from_sandbox`, `list_attached`, and `get_sandbox_provider_environment` with:

```rust
pub async fn list_providers_by_prefix(
    client: &mut OpenShellClient<Channel>,
    prefix: &str,
) -> Result<Vec<Provider>, ProviderError> {
    let resp = client
        .list_providers(proto_v1::ListProvidersRequest {
            // 0 = server default (full list); explicit pagination is not
            // required for typical per-agent fan-out (< few dozen).
            limit: 0,
            offset: 0,
        })
        .await
        .map_err(|s| classify_status(s, "<list>"))?
        .into_inner();
    let mut out = Vec::with_capacity(resp.providers.len());
    for p in resp.providers {
        let name = p
            .metadata
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or_default();
        if name.starts_with(prefix) {
            out.push(provider_from_proto(p));
        }
    }
    Ok(out)
}

pub async fn attach_to_sandbox(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    provider_name: &str,
) -> Result<(), ProviderError> {
    let req = proto_v1::AttachSandboxProviderRequest {
        sandbox_name: sandbox_name.to_string(),
        provider_name: provider_name.to_string(),
        expected_resource_version: 0,
    };
    client
        .attach_sandbox_provider(req)
        .await
        .map_err(|s| classify_status(s, provider_name))?;
    Ok(())
}

pub async fn detach_from_sandbox(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
    provider_name: &str,
) -> Result<(), ProviderError> {
    let req = proto_v1::DetachSandboxProviderRequest {
        sandbox_name: sandbox_name.to_string(),
        provider_name: provider_name.to_string(),
        expected_resource_version: 0,
    };
    client
        .detach_sandbox_provider(req)
        .await
        .map_err(|s| classify_status(s, provider_name))?;
    Ok(())
}

pub async fn list_attached(
    client: &mut OpenShellClient<Channel>,
    sandbox_name: &str,
) -> Result<Vec<String>, ProviderError> {
    let req = proto_v1::ListSandboxProvidersRequest {
        sandbox_name: sandbox_name.to_string(),
    };
    let resp = client
        .list_sandbox_providers(req)
        .await
        .map_err(|s| classify_status(s, sandbox_name))?
        .into_inner();
    Ok(resp
        .providers
        .into_iter()
        .filter_map(|p| p.metadata.map(|m| m.name))
        .collect())
}

pub async fn get_sandbox_provider_environment(
    client: &mut OpenShellClient<Channel>,
    sandbox_id: &str,
) -> Result<HashMap<String, String>, ProviderError> {
    let req = proto_v1::GetSandboxProviderEnvironmentRequest {
        sandbox_id: sandbox_id.to_string(),
    };
    let resp = client
        .get_sandbox_provider_environment(req)
        .await
        .map_err(|s| classify_status(s, sandbox_id))?
        .into_inner();
    Ok(resp.environment)
}
```

- [ ] **Step 4: Delete the dead CLI helpers**

In `crates/right-openshell/src/providers.rs`, delete:

- `ensure_v2_enabled` function and `V2EnableResult` struct
- `get_v2_flag` function and `classify_v2_get_output` helper
- `run_cli` function
- `parse_provider_json` function
- `provider_from_json` function
- `stderr_is_not_found` function
- The `ProviderError::Cli` enum variant
- The `ProviderError::V2NotEnabled` enum variant
- The `use tokio::process::Command;` import (if still present)
- The `use std::process::Stdio;` import (if still present)

Also remove the `#[allow(unused_imports)]` from Task 7 step 4 if it was added.

- [ ] **Step 5: Run all provider tests**

```bash
devenv shell -- cargo test -p right-openshell providers_tests
```

Expected: 13 passed, 0 failed (the 8 from Task 7 + the 5 added in this task).

- [ ] **Step 6: Confirm the crate compiles**

```bash
devenv shell -- cargo build -p right-openshell
```

Expected: success. `right-openshell` is now CLI-free for providers; the downstream `right` crate (which Task 9 updates) still fails to compile because it calls the old signatures — that's fine, this commit is a partial migration.

- [ ] **Step 7: Commit**

```bash
git add crates/right-openshell/src/providers.rs \
        crates/right-openshell/src/providers_tests.rs
git commit -m "refactor(providers): finish gRPC migration; drop CLI helpers

Migrates list_providers_by_prefix, attach_to_sandbox, detach_from_sandbox,
list_attached, get_sandbox_provider_environment to gRPC. Removes
ensure_v2_enabled and V2EnableResult (providers_v2_enabled is no longer
a config flag in v0.0.50 — providers are unconditionally enabled).
Deletes run_cli, parse_provider_json, provider_from_json,
stderr_is_not_found, classify_v2_get_output, ProviderError::Cli and
ProviderError::V2NotEnabled.

internal_api_providers.rs still references old signatures; updates in
the next commit."
```

---

## Task 9: Update call sites in `internal_api_providers.rs`

**Files:**
- Modify: `crates/right/src/internal_api_providers.rs`

The provider functions now take `&mut OpenShellClient<Channel>` instead of `&GatewayEndpoint`. Every handler in this file that previously did `let endpoint = resolve_gateway_endpoint().await?;` followed by `providers::foo(&endpoint, ...)` must instead:

1. Resolve the mTLS dir: `let mtls_dir = right_openshell::openshell::default_mtls_dir();`
2. Open a client once: `let mut client = right_openshell::openshell::connect_grpc(&mtls_dir).await.map_err(...)?;`
3. Pass `&mut client` to every provider call.

There are several handlers that need this treatment. Update each in turn.

- [ ] **Step 1: Drop the `V2NotEnabled` variant and add a client-open helper**

In `crates/right/src/internal_api_providers.rs`:

1. **Remove** the `V2NotEnabled` variant from `ProviderApiError` (lines 17-18 of the current file) and the matching arm in the `IntoResponse` impl (line 37). The gateway no longer has a `providers_v2_enabled` flag in v0.0.50.

2. **Add** the client-open helper near the top of the file, below the `IntoResponse` impl:

```rust
/// Open one gRPC client per request. The returned client wraps a
/// tonic::Channel (internally Arc-shared) and is threaded through every
/// provider call in this handler.
async fn open_openshell_client()
-> Result<
    right_openshell::openshell_proto::openshell::v1::open_shell_client::OpenShellClient<
        tonic::transport::Channel,
    >,
    ProviderApiError,
> {
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .map_err(|e| ProviderApiError::Gateway(format!("connect: {e:#}")))
}
```

The existing `ProviderApiError::Gateway(String)` variant is reused for "failed to talk to OpenShell".

- [ ] **Step 2: Replace each `resolve_gateway_endpoint`-using block**

For each handler that contains a `let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await...;` line (approximately at lines 350, 479, 754, 968, 1079, 1268 of `internal_api_providers.rs` per the spec exploration — confirm by `grep -n "resolve_gateway_endpoint" crates/right/src/internal_api_providers.rs`), do the following:

1. **Delete** the `resolve_gateway_endpoint` line.
2. **Add** `let mut client = open_openshell_client().await?;` in its place.
3. **Replace** every `providers::foo(&endpoint, ...)` in the surrounding handler with `providers::foo(&mut client, ...)`.

Concretely, every site like:

```rust
let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await?;
// ...
right_openshell::providers::get_provider(&endpoint, &entry.name).await
```

becomes:

```rust
let mut client = open_openshell_client().await?;
// ...
right_openshell::providers::get_provider(&mut client, &entry.name).await
```

The 9 functions to retarget at every call site are:

| Old: takes `&endpoint` | New: takes `&mut client` |
|---|---|
| `providers::create_provider(&endpoint, &spec)` | `providers::create_provider(&mut client, &spec)` |
| `providers::get_provider(&endpoint, &name)` | `providers::get_provider(&mut client, &name)` |
| `providers::update_provider(&endpoint, &spec)` | `providers::update_provider(&mut client, &spec)` |
| `providers::delete_provider(&endpoint, &name)` | `providers::delete_provider(&mut client, &name)` |
| `providers::list_providers_by_prefix(&endpoint, &prefix)` | `providers::list_providers_by_prefix(&mut client, &prefix)` |
| `providers::attach_to_sandbox(&endpoint, &sbox, &name)` | `providers::attach_to_sandbox(&mut client, &sbox, &name)` |
| `providers::detach_from_sandbox(&endpoint, &sbox, &name)` | `providers::detach_from_sandbox(&mut client, &sbox, &name)` |
| `providers::list_attached(&endpoint, &sbox)` | `providers::list_attached(&mut client, &sbox)` |
| `providers::get_sandbox_provider_environment(&endpoint, &sbox_id)` | `providers::get_sandbox_provider_environment(&mut client, &sbox_id)` |

Also delete any remaining `right_openshell::providers::ensure_v2_enabled(&endpoint).await...` calls — that function no longer exists.

Use this command to enumerate every site that still needs updating:

```bash
devenv shell -- cargo build -p right 2>&1 | grep -E 'expected.*OpenShellClient|expected.*Channel' | head -50
```

Iterate until the build is clean.

- [ ] **Step 3: Update `bot::lib` and `right_bot` call sites if they exist**

```bash
grep -rn "providers::\(create_provider\|get_provider\|update_provider\|delete_provider\|list_providers_by_prefix\|attach_to_sandbox\|detach_from_sandbox\|list_attached\|get_sandbox_provider_environment\|ensure_v2_enabled\)" --include="*.rs" -- crates/bot crates/right-agent
```

For each match outside `internal_api_providers.rs`, apply the same `&endpoint` → `&mut client` substitution. Open one client at the top of the calling scope.

- [ ] **Step 4: Update `crates/bot/src/lib.rs` — reconcile path**

The reconcile path (`reconcile_for_sandbox` or `right_openshell::providers::reconcile_for_sandbox`) takes a sandbox name and currently constructs its own endpoint. Update its signature to take `&mut OpenShellClient<Channel>` instead. Confirm via:

```bash
grep -n "reconcile_for_sandbox" crates/right-openshell/src/providers.rs crates/bot/src/lib.rs
```

Apply the `&endpoint` → `&mut client` substitution; the bot caller opens a client before calling it.

- [ ] **Step 5: Confirm the workspace builds**

```bash
devenv shell -- cargo build --workspace
```

Expected: success.

- [ ] **Step 6: Run the existing provider-adjacent tests**

```bash
devenv shell -- cargo test -p right --lib internal_api_providers
devenv shell -- cargo test -p right-openshell providers_tests
```

Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add crates/right/src/internal_api_providers.rs \
        crates/bot/src/lib.rs \
        crates/right-openshell/src/providers.rs
# (the providers.rs may have minor changes from reconcile signature update)
git commit -m "refactor(internal_api_providers): pass gRPC client to provider fns

Each handler now opens one OpenShellClient<Channel> via the new
open_openshell_client() helper and threads it through provider calls.
Drops all resolve_gateway_endpoint usage in this file and the now-empty
ensure_v2_enabled call sites."
```

---

## Task 10: Wire `openshell_preflight` into bot startup

**Files:**
- Modify: `crates/bot/src/lib.rs`

- [ ] **Step 1: Locate the existing `upgrade::check` call**

```bash
grep -n "upgrade::check\|checking for claude upgrade" crates/bot/src/lib.rs
```

Note the line where `upgrade::check(...)` is invoked; the preflight runs **before** it, so a too-old gateway is reported before we even probe Claude.

- [ ] **Step 2: Add the preflight call**

In `crates/bot/src/lib.rs`, locate the function that initializes the bot (likely `run` or similar — confirm in step 1). Before the `upgrade::check` invocation, insert:

```rust
{
    let mtls_dir = right_openshell::openshell::default_mtls_dir();
    let mut client = right_openshell::openshell::connect_grpc(&mtls_dir)
        .await
        .map_err(|e| miette::miette!("openshell gRPC connect failed: {e:#}"))?;
    if let Err(e) = right_openshell::preflight::openshell_preflight(&mut client).await {
        tracing::error!(error = %e, "OpenShell version preflight failed; refusing to start");
        return Err(miette::miette!("{e}"));
    }
    tracing::info!("OpenShell version preflight passed");
}
```

If the surrounding function returns `Result<(), some_other_error>`, adapt the `Err` arm accordingly. The pattern: log the typed error via Display, then convert into the surrounding error type.

- [ ] **Step 3: Verify the bot still builds**

```bash
devenv shell -- cargo build -p right-bot
```

Expected: success.

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/lib.rs
git commit -m "feat(bot): version preflight on startup

Calls right_openshell::preflight::openshell_preflight before
upgrade::check so a too-old CLI or gateway is reported with an
actionable error before any other startup work begins."
```

---

## Task 11: Update existing ci_openshell tests

**Files:**
- Modify: `crates/right-openshell/tests/ci_openshell_provider.rs`

The CI tests still hold `&endpoint` and call `ensure_v2_enabled`. They must be updated to the new signatures. They remain `#[ignore = "ci-openshell: ..."]`.

- [ ] **Step 1: Update each `#[ignore]` provider test**

Open `crates/right-openshell/tests/ci_openshell_provider.rs`. For each test:

1. Delete the `let _ = ensure_v2_enabled(&endpoint).await.unwrap();` line if present.
2. Replace `let endpoint = right_openshell::openshell::resolve_gateway_endpoint().await...;` with:

   ```rust
   let mtls_dir = right_openshell::openshell::default_mtls_dir();
   let mut client = right_openshell::openshell::connect_grpc(&mtls_dir).await.unwrap();
   ```

3. Replace every `&endpoint` provider-function argument with `&mut client`.
4. Delete the `ci_openshell_provider_v2_flip` test entirely — its target function (`ensure_v2_enabled`) is gone.

- [ ] **Step 2: Confirm the test file compiles**

```bash
devenv shell -- cargo build --tests -p right-openshell
```

Expected: success. The tests stay `#[ignore]`; they do not run in this step.

- [ ] **Step 3: Commit**

```bash
git add crates/right-openshell/tests/ci_openshell_provider.rs
git commit -m "test(ci-openshell): update provider tests for gRPC signatures

Drops ensure_v2_enabled calls (function removed) and switches every
provider call from &endpoint to &mut OpenShellClient<Channel>. Removes
ci_openshell_provider_v2_flip — target function gone. Other tests stay
#[ignore = \"ci-openshell: ...\"] per AGENTS.md."
```

---

## Task 12: Update architecture docs (cite-on-touch)

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/providers.md`
- Modify: `docs/architecture/sandbox.md`

- [ ] **Step 1: Update `ARCHITECTURE.md`**

In `ARCHITECTURE.md`, locate the "OpenShell Integration Conventions" section. Replace the bullet about "Prefer gRPC over CLI" with:

```markdown
- **Use gRPC for everything except file transfer and `policy set --wait`**:
  every provider control-plane operation (Create/Get/Update/Delete/List/
  Attach/Detach/ListAttached/GetSandboxProviderEnvironment) goes through
  tonic-generated client stubs from the vendored `openshell.v1` proto.
  CLI remains only for: SSH+tar-backed file upload/download, and
  `openshell policy set --wait` (the policy hot-apply path). Adding a new
  provider operation by CLI is a review-blocking defect.
```

Locate the "Vendored proto compatibility is load-bearing" gotcha (around line 561) and rewrite the version reference:

```markdown
- **Vendored proto compatibility is load-bearing**: OpenShell `v0.0.50`
  is the minimum supported version (CLI and gateway).
  `crates/right-openshell/proto/UPSTREAM.md` records the pinned tag and
  fetch date. To bump: run `scripts/vendor-openshell-proto.sh <tag>` and
  `cargo check -p right-openshell` to regenerate stubs. Older protos
  lacked `AttachSandboxProvider` / `DetachSandboxProvider` /
  `ListSandboxProviders` RPCs — the providers feature depends on them.
  `right_openshell::preflight::openshell_preflight` enforces this at
  bot startup; both CLI and gateway must report
  `>= MIN_OPENSHELL_VERSION`.
```

- [ ] **Step 2: Update `docs/architecture/providers.md`**

Rewrite the operations narrative so it describes the gRPC call shapes (refer to the function table in `crates/right-openshell/src/providers.rs`) instead of CLI commands. Replace any `openshell provider` / `openshell sandbox provider` examples with the RPC names (`CreateProvider`, `AttachSandboxProvider`, etc.) and the request/response message types from `openshell.v1`.

If the file is large and your scope is unclear, find the section that mentions "openshell provider create" or "--credential" and treat that as the boundary of what to update.

- [ ] **Step 3: Update `docs/architecture/sandbox.md`**

Locate the bot-startup sandbox sequence narrative. Add a new step near the top: `right_openshell::preflight::openshell_preflight(&mut client).await?` runs before any sandbox interaction; it spawns `openshell --version` and issues a `Health` RPC against the gateway; a too-old version fails the process with an actionable diagnostic.

- [ ] **Step 4: Confirm files are still under their size budgets**

```bash
wc -c ARCHITECTURE.md
```

Expected: under 40000 (per the AGENTS.md hard budget). If close to the limit, move new descriptive prose into the satellite files and leave only the rule statements in `ARCHITECTURE.md`.

- [ ] **Step 5: Commit**

```bash
git add ARCHITECTURE.md docs/architecture/providers.md docs/architecture/sandbox.md
git commit -m "docs(arch): cite-on-touch — gRPC provider migration + preflight

Updates ARCHITECTURE.md provider/openshell rules, the providers
narrative, and the sandbox startup sequence to reflect the post-v0.0.50
gRPC-first surface and the openshell_preflight gate."
```

---

## Task 13: Final workspace verification

- [ ] **Step 1: Run the full workspace test suite**

```bash
devenv shell -- cargo test --workspace
```

Expected: all tests pass. Newly-added `providers_tests` (13) and `preflight_tests` (15) run on every invocation; `ci_openshell_provider_*` remain `#[ignore]` and are skipped by default.

If any pre-existing tests fail, capture which ones and confirm they failed before this work began (compare to the baseline from the worktree start). Do not chase pre-existing failures; the failure must be one this work introduced.

- [ ] **Step 2: Run clippy**

```bash
devenv shell -- cargo clippy --workspace --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 3: Manual smoke test of `right up`**

```bash
devenv shell -- cargo run -p right -- up
```

Expected: startup completes, log line `OpenShell version preflight passed` appears. The previous warning `WARN right_bot: provider reconcile failed: openshell CLI ... exited 2: error: unexpected argument '--output' found` is gone.

If the host's installed `openshell` binary is still v0.0.42, the preflight WILL fail with `openshell CLI is 0.0.42, need >= 0.0.50; run 'brew upgrade openshell' (or your platform equivalent)`. That's the correct behavior — upgrade the binary and re-run.

- [ ] **Step 4: Final commit**

If any minor adjustments were made (formatting, missed imports, etc.), commit them with a chore message:

```bash
git add -A
git commit -m "chore: workspace tidy after gRPC migration"
```

Otherwise skip this step.
