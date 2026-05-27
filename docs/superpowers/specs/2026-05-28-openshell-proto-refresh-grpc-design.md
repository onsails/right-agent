# OpenShell proto refresh + provider gRPC migration

**Status:** Design — pending plan
**Date:** 2026-05-28
**Worktree:** `feat/providers`

## Problem

The providers feature shipped against an assumed `openshell` CLI surface that does not exist in installed v0.0.42. Every provider control-plane call site uses `--output json` and other flags the CLI rejects. The startup reconcile path fails on first invocation:

```
WARN right_bot: provider reconcile failed:
openshell CLI "openshell sandbox provider list" exited 2:
error: unexpected argument '--output' found
```

The vendored proto under `crates/right-openshell/proto/openshell/` was snapshotted on 2026-04-07 (commit `0eaa275f`). It predates the gateway providers feature: it has `CreateProvider` / `GetProvider` / `ListProviders` / `UpdateProvider` / `DeleteProvider` / `GetSandboxProviderEnvironment`, but no `AttachSandboxProvider` / `DetachSandboxProvider` / `ListSandboxProviders` RPCs. The v0.0.42 CLI exposes attach/detach/list-attached anyway — it must be hitting RPCs the vendored proto doesn't show. Net: we have no first-class view of the current gateway surface.

Tests didn't catch any of this because all `ci_openshell_provider_*` integration tests are `#[ignore = "ci-openshell: ..."]` per AGENTS.md and run only in CI, and the failing call sites (`list_attached`, `list_providers_by_prefix`) had no test coverage at all.

## Goal

After this change:

1. The vendored proto matches `NVIDIA/OpenShell@v0.0.50` (current latest).
2. Every provider control-plane call goes through gRPC. CLI is used only where no gRPC RPC exists (file transfer, `policy set --wait`).
3. Bot startup hard-fails if either the installed CLI or the running gateway is older than `MIN_OPENSHELL_VERSION` (`v0.0.50`), with an actionable diagnostic.
4. Mock-gRPC unit tests run on every `cargo test --workspace`, eliminating the class of "code assumes a non-existent CLI surface" bug.

## Non-goals

- Migrating to `RotateProviderCredential` (we currently use `UpdateProvider` with new credentials).
- Provider profiles (`ListProviderProfiles`, `ImportProviderProfiles`, etc.).
- Provider refresh control plane (`ConfigureProviderRefresh`, `GetProviderRefreshStatus`).
- Refactoring `crates/right/src/internal_api_providers.rs` beyond signature-driven changes.

## Architecture

Three crate-level changes; no new crate.

### 1. `crates/right-openshell/proto/` — re-vendored from v0.0.50

A new `scripts/vendor-openshell-proto.sh <tag>` re-pulls `datamodel.proto`, `sandbox.proto`, `openshell.proto` from `https://raw.githubusercontent.com/NVIDIA/OpenShell/<tag>/proto/`. `crates/right-openshell/proto/UPSTREAM.md` records the pinned tag and fetch date. Existing tonic-prost-build pipeline regenerates client stubs automatically.

`UPSTREAM.md` format:

```
tag: v0.0.50
fetched: 2026-05-28T13:00:00Z
```

Optional CI gate: re-run the script and `git diff --exit-code` to catch hand-edits.

### 2. `crates/right-openshell/src/providers.rs` — gRPC rewrite

Free-function API kept verbatim (approach A from brainstorm). Existing call sites in `crates/right/src/internal_api_providers.rs` don't change. Each function opens a tonic `Channel` via `connect_grpc(&mtls_dir)`, calls the RPC, maps prost types to existing `Provider` / `ProviderSpec` structs.

Single-sourced connect helper:

```rust
async fn connect_provider_client(
    endpoint: &GatewayEndpoint,
) -> Result<OpenShellClient<Channel>, ProviderError> {
    crate::openshell::connect_grpc(&endpoint.mtls_dir)
        .await
        .map_err(|e| ProviderError::Grpc(format!("connect: {e:#}")))
}
```

Function → RPC mapping:

| Function | RPC |
|---|---|
| `create_provider` | `CreateProvider(CreateProviderRequest)` |
| `get_provider` | `GetProvider(GetProviderRequest)` |
| `update_provider` | `UpdateProvider(UpdateProviderRequest)` |
| `delete_provider` | `DeleteProvider(DeleteProviderRequest)` |
| `list_providers_by_prefix` | `ListProviders(ListProvidersRequest)` + client-side prefix filter |
| `attach_to_sandbox` | `AttachSandboxProvider(AttachSandboxProviderRequest)` |
| `detach_from_sandbox` | `DetachSandboxProvider(DetachSandboxProviderRequest)` |
| `list_attached` | `ListSandboxProviders(ListSandboxProvidersRequest)` |
| `get_sandbox_provider_environment` | `GetSandboxProviderEnvironment(...)` |

Deleted code:

- `run_cli` helper
- `parse_provider_json`, `provider_from_json`, `classify_v2_get_output`, `stderr_is_not_found`
- All `--output json` argv plumbing
- `--credential KEY` env-injection trick (gRPC carries credentials inside the request body)
- `ensure_v2_enabled` (the `providers_v2_enabled` config flag does not appear in v0.0.50 protos — providers are unconditionally enabled — so this function and all `UpdateConfig`-based feature-flag plumbing is removed; its callers drop the `let _ = ensure_v2_enabled(&endpoint).await.unwrap();` line)

Preserved:
- Manual `Debug` impl on `ProviderSpec` (redacts credentials map count, e.g. `<2 redacted>`)
- `SecretString` for in-memory credential transport
- `ProviderError::NotFound` variant
- `ReconcileReport { errors: Vec<(String, String)> }` partial-failure semantics

### 3. Startup version preflight

New const, single-sourced in `right-openshell::openshell`:

```rust
pub const MIN_OPENSHELL_VERSION: semver::Version = semver::Version::new(0, 0, 50);
```

New `pub async fn openshell_preflight(endpoint: &GatewayEndpoint) -> Result<(), PreflightError>`:

1. Run `openshell --version`, parse `^openshell (\d+\.\d+\.\d+)$`, compare to `MIN_OPENSHELL_VERSION`.
2. gRPC `Health(HealthRequest)`, read `version` field, semver-parse, compare.
3. Either failing case returns a typed `PreflightError` variant.

Wired into bot startup in `crates/bot/src/lib.rs` adjacent to the existing `upgrade::check` (CC version probe). Hard-fails the process on failure with `tracing::error!` and a non-zero exit code. No quiet degradation.

## Components

### 2.1 `scripts/vendor-openshell-proto.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
TAG="${1:?usage: vendor-openshell-proto.sh <tag>  (e.g. v0.0.50)}"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
DEST="crates/right-openshell/proto/openshell"
for f in datamodel.proto sandbox.proto openshell.proto; do
  curl -fsSL "https://raw.githubusercontent.com/NVIDIA/OpenShell/$TAG/proto/$f" -o "$TMP/$f"
done
rm -f "$DEST"/*.proto
mv "$TMP"/*.proto "$DEST/"
printf 'tag: %s\nfetched: %s\n' "$TAG" "$(date -u +%FT%TZ)" > "$DEST/../UPSTREAM.md"
echo "Vendored openshell proto from $TAG"
```

### 2.2 `PreflightError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum PreflightError {
    #[error("openshell CLI binary not found on PATH")]
    CliMissing,
    #[error("could not parse `openshell --version` output: {0}")]
    CliVersionUnparseable(String),
    #[error("openshell CLI is {found}, need ≥{required}; run `brew upgrade openshell` (or your platform equivalent)")]
    CliTooOld { found: Version, required: Version },
    #[error("openshell gateway unreachable: {0}")]
    GatewayUnreachable(tonic::Status),
    #[error("could not parse openshell gateway version: {0}")]
    GatewayVersionUnparseable(String),
    #[error("openshell gateway is {found}, need ≥{required}; upgrade your gateway")]
    GatewayTooOld { found: Version, required: Version },
}
```

### 2.3 Mock gRPC server (test harness)

Extends the existing `crates/right-openshell/src/openshell_tests.rs` `OpenShellService` impl with provider RPC handlers. Per-test override pattern (closures or per-test struct fields); `unimplemented!()` defaults for everything else.

Test files:

- `crates/right-openshell/src/providers_tests.rs` — provider RPC unit tests
- `crates/right-openshell/src/preflight_tests.rs` — version preflight unit tests

Both run on `cargo test --workspace`, no `#[ignore]`.

## Data flow

### Provider create (dashboard → reconcile)

```
Dashboard POST /providers
  → internal_api_providers::handle_provider_create
    → validate inputs                              (unchanged)
    → providers::create_provider(endpoint, &spec)  (now gRPC)
        → tonic::Channel via connect_grpc()
        → CreateProvider(CreateProviderRequest{ provider: Provider{
            metadata: ObjectMeta{ name },
            spec: ProviderSpec{ type_, credentials, config }
          }})
        → returns ProviderResponse{ provider: Provider }
    → write agent.yaml::sandbox::providers           (unchanged)
    → write policy.yaml stanza + policy set --wait   (unchanged)
    → providers::attach_to_sandbox(...)              (now gRPC)
        → AttachSandboxProvider(AttachSandboxProviderRequest{ sandbox_name, provider_name })
    → respond 200
```

Rollback paths (create → attach → policy) keep their `if let Err(rollback_err) = ... { tracing::warn!(...) }` pattern. Rollback failures log; do not mask the original error.

### Bot startup

```
right up
  → openshell_preflight(endpoint)                       [NEW]
      → openshell --version → semver cmp ≥ MIN_OPENSHELL_VERSION
      → Health(HealthRequest)   → semver cmp ≥ MIN_OPENSHELL_VERSION
      → hard fail with actionable diagnostic if either is short
  → existing sandbox bring-up                            (unchanged)
  → reconcile_for_sandbox(agent, sandbox)
      → list_attached(endpoint, sandbox)                 (now gRPC)
          → ListSandboxProviders(...)
      → for providers in agent.yaml but not attached:
          → list_providers_by_prefix(endpoint, "<agent>-")  (now gRPC)
              → ListProviders(...) → client-side filter
          → attach_to_sandbox(...)                       (now gRPC)
      → for providers attached but not in agent.yaml: detach (now gRPC)
      → returns ReconcileReport{ errors }                (unchanged)
```

### Credential transport

CLI today: value goes to `cmd.env(KEY, VALUE)`, `--credential KEY` references it. Trick keeps value out of `/proc/<pid>/cmdline`.

gRPC tomorrow: credentials inside `Provider.spec.credentials` (`map<string, string>`). Cross the wire over mTLS to the gateway. No child process, no `/proc/<pid>/cmdline` exposure, no log exposure (manual `Debug` impl on `ProviderSpec` redacts the map). Strictly better than the CLI path.

### NotFound mapping

CLI today: stderr substring scan (`"not found" | "NotFound" | "not attached"`) in `stderr_is_not_found`.

gRPC: read `tonic::Status::code()` directly. `tonic::Code::NotFound` → `ProviderError::NotFound(name)`. Other codes → `ProviderError::Grpc(format!("{}: {}", code, message))`. Substring scan removed entirely.

## Error handling

`ProviderError` enum kept; `Cli { cmd, status, stderr }` variant deleted. All gRPC failure paths absorbed by `Grpc(String)` variant. New `PreflightError` enum lives next to `MIN_OPENSHELL_VERSION` in `right-openshell::openshell`.

Mapping table:

| Source | Mapped to |
|---|---|
| `tonic::transport::Error` (handshake, connect) | `ProviderError::Grpc(format!("connect: {e:#}"))` |
| `tonic::Status` with `code() == NotFound` | `ProviderError::NotFound(name)` |
| `tonic::Status` other codes | `ProviderError::Grpc(format!("{}: {}", status.code(), status.message()))` |
| Prost field missing / parse failure | `ProviderError::Grpc(format!("parse {field}: {context}"))` |

Use `format!("{e:#}")` (alternate Display) per CLAUDE-base.md so the full anyhow chain survives string conversion.

FAIL FAST preserved:

- `reconcile_for_sandbox` keeps the `ReconcileReport.errors` collector (per-provider errors don't `?`-propagate).
- Rollback sites keep `if let Err(rollback_err) = ... { tracing::warn!(...) }`.
- Direct write paths in `handle_provider_create` / `handle_provider_remove` propagate `ProviderError` with `?`.

Tonic transport vs RPC errors stay distinguishable in logs by the `"connect:"` vs `"{code}:"` prefix on the `Grpc(String)` payload.

## Testing

### Mock gRPC server unit tests

`crates/right-openshell/src/providers_tests.rs` — coverage matrix:

| Test | What it pins |
|---|---|
| `create_provider_sends_typed_request` | Request payload matches `CreateProviderRequest` shape; credentials map populated |
| `create_provider_returns_not_found_on_status_code` | `tonic::Code::NotFound` → `ProviderError::NotFound` |
| `create_provider_returns_grpc_on_other_status` | Other codes → `ProviderError::Grpc("{code}: {msg}")` |
| `get_provider_not_found` | NotFound mapping for get |
| `update_provider_round_trip` | Update payload + response parsing |
| `delete_provider_not_found_is_idempotent_at_caller` | Caller-level idempotency (matches `handle_provider_remove` iter-2 behavior) |
| `list_providers_by_prefix_filters_client_side` | All-listing + prefix filter logic |
| `attach_to_sandbox_sends_typed_request` | sandbox_name + provider_name plumbed correctly |
| `detach_from_sandbox_not_found` | NotFound mapping for detach |
| `list_attached_returns_names` | Decoder reads `Sandbox.providers` correctly |
| `get_sandbox_provider_environment_returns_map` | Env map decoded as `HashMap<String,String>` |
| `credential_value_never_appears_in_request_debug` | `format!("{:?}", request)` — credential value absent |

### Preflight unit tests

`crates/right-openshell/src/preflight_tests.rs`:

| Test | What it pins |
|---|---|
| `preflight_parses_openshell_version` | Regex matches `openshell 0.0.50`; rejects malformed |
| `preflight_fails_on_old_cli` | `0.0.42 < 0.0.50` → `CliTooOld` |
| `preflight_fails_on_old_gateway` | Mock `Health` returns `0.0.49` → `GatewayTooOld` |
| `preflight_succeeds_on_exact_min` | `0.0.50 == 0.0.50` passes |
| `preflight_succeeds_on_newer` | `0.0.51 > 0.0.50` passes |

CLI version test uses a captured-stdout fake (parser takes `&str`).

### Existing CI integration tests

`crates/right-openshell/tests/ci_openshell_provider.rs` tests stay `#[ignore = "ci-openshell: ..."]` per AGENTS.md. Their assertions don't change; they exercise the same API surface, now backed by gRPC.

### Cadence

Targeted intermediate verification per AGENTS.md:
- After re-vendoring proto: `devenv shell -- cargo check -p right-openshell` confirms generated code compiles.
- After each `providers.rs` function migration: `devenv shell -- cargo test -p right-openshell providers_tests::<name>`.
- After preflight wiring: `devenv shell -- cargo test -p right-openshell preflight_tests`.

Final verification at worktree completion: `devenv shell -- cargo test --workspace` (mandatory per AGENTS.md).

## Open questions

None — `ensure_v2_enabled` resolved to "remove unconditionally" after confirming v0.0.50 protos drop the `providers_v2_enabled` flag.

## ARCHITECTURE.md impact

Updates needed (cite-on-touch):

- `ARCHITECTURE.md` "OpenShell Integration Conventions" — flip "Prefer gRPC over CLI" guidance to "gRPC for everything except file transfer and `policy set --wait`" once this lands.
- `ARCHITECTURE.md` "Vendored proto compatibility is load-bearing" — bump version note from `v0.0.42` to `v0.0.50`. Update gotcha that referenced `Sandbox.metadata.id` (still load-bearing); also describe the vendor script as the bump procedure.
- `docs/architecture/providers.md` — refresh the provider operations narrative from CLI to gRPC.
- `docs/architecture/sandbox.md` — note the new `openshell_preflight` step in startup sequence.
