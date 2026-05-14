# Dependency Audit Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut cold-build wall time and remove dead dependencies by unifying on a single rustls crypto backend (ring), pruning unused per-crate deps left over from the right-core decomposition, tightening tonic features, and silencing cargo-machete false positives so the audit stays runnable.

**Architecture:** Four atomic commits — P0 swaps `reqwest`'s rustls feature flag to drop the `aws-lc-sys` C build and explicitly installs `ring` as the rustls default crypto provider at the single binary entrypoint. P1 prunes per-crate unused deps confirmed via grep. P2 tightens `tonic`'s feature set. P3 adds `cargo-machete` ignore entries for `tonic::include_proto!` false positives. Each commit is independently revertable.

**Tech Stack:** Cargo workspace (edition 2024), `rustls 0.23` (ring provider), `reqwest 0.13`, `tonic 0.14`, `cargo-machete` (run via `nix run nixpkgs#cargo-machete`).

**Worktree (suggested):** Create one with `superpowers:using-git-worktrees` so the audit work doesn't tangle with the in-progress `master` state:
```bash
git worktree add .worktrees/dep-audit -b dep-audit master
cd .worktrees/dep-audit
```

**Pre-flight baseline** (capture before starting so you can verify deltas):

```bash
devenv shell -- bash -lc 'cargo tree -i aws-lc-sys 2>&1 | tee /tmp/before-aws-lc.txt'
devenv shell -- bash -lc 'cargo tree -i ring 2>&1 | tee /tmp/before-ring.txt'
time devenv shell -- bash -lc 'cargo clean -p reqwest -p rustls -p hyper-rustls -p tokio-rustls -p rustls-platform-verifier -p aws-lc-sys -p aws-lc-rs -p ring && cargo check --workspace --tests' 2>&1 | tail -5 | tee /tmp/before-time.txt
```

The `before-aws-lc.txt` must show `aws-lc-sys v0.39` pulled via `reqwest v0.13.3 → rustls-platform-verifier`. If it does not, stop — the diagnosis behind this plan is stale, redo the audit.

---

## Task 1: P0 — drop the aws-lc-rs crypto backend

**Files:**
- Modify: `Cargo.toml` (workspace dependencies, line containing `reqwest = …`)
- Modify: `crates/right/src/main.rs` (top of `async fn main`)
- Modify: `crates/right/Cargo.toml` (add `rustls` direct dep so we can call `crypto::ring::default_provider()`)

### Background

`cargo tree -e features -i rustls-platform-verifier` shows the chain:

```
rustls-platform-verifier → reqwest 0.13.3 feature "rustls"
                                          ↓
                       feature "__rustls-aws-lc-rs"
                                          ↓
                                  aws-lc-rs → aws-lc-sys (C build, ~2 min cold)
```

reqwest 0.13's bare `"rustls"` feature implicitly means **aws-lc-rs**, which is the only rustls 0.23 crypto provider it auto-installs. reqwest 0.12 (transitively, via `teloxide-core 0.13`) already enables `__rustls-ring`, so rustls is compiled with both providers — wasted compile time and ambiguous default-provider semantics at runtime. Reqwest 0.13 offers `rustls-no-provider` for callers that want to pick their own provider — we use that and install `ring` ourselves so rustls 0.23 has exactly one provider in the graph (ring).

### Steps

- [ ] **Step 1.1: Verify the current state**

Run:
```bash
devenv shell -- bash -lc 'cargo tree -i aws-lc-sys'
```
Expected output: a tree rooted at `aws-lc-sys v0.39.0` → `aws-lc-rs v1.16.x` → `rustls v0.23.x`. If `aws-lc-sys` is reported as not present, this plan is already partially applied; investigate before continuing.

- [ ] **Step 1.2: Change reqwest feature in workspace deps**

In the root `Cargo.toml`, replace the existing line:
```toml
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls", "form"] }
```
with:
```toml
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls-no-provider", "form"] }
```

- [ ] **Step 1.3: Add `rustls` as a direct workspace dep, ring-only**

In the root `Cargo.toml`'s `[workspace.dependencies]` section, append:
```toml
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12", "logging"] }
```
(Workspace-level entry — we want a single rustls version pin.)

- [ ] **Step 1.4: Pull `rustls` into the `right` binary crate**

In `crates/right/Cargo.toml`'s `[dependencies]` section (preserving alphabetical order), add:
```toml
rustls = { workspace = true }
```

- [ ] **Step 1.5: Install ring as the default crypto provider on startup**

In `crates/right/src/main.rs`, locate `async fn main() -> miette::Result<()>` (search for `async fn main`). Make the first executable statement inside its body:

```rust
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| miette::miette!("rustls ring crypto provider already installed"))?;
```

Place it **before** any other code in `main()`. `install_default` returns `Err(())` if a provider was already installed (e.g. by tests in the same process) — for the binary entrypoint we treat that as fatal.

- [ ] **Step 1.6: Verify the dep graph collapsed**

Run:
```bash
devenv shell -- bash -lc 'cargo tree -i aws-lc-sys 2>&1' && echo SHOULD_FAIL
devenv shell -- bash -lc 'cargo tree -i aws-lc-rs 2>&1'  && echo SHOULD_FAIL
```
Expected: each command prints `error: package ID specification \`aws-lc-sys\` did not match any packages` (or equivalent "not found" message). `SHOULD_FAIL` must NOT appear after either line. If it does appear, some dep is still pulling aws-lc-rs — likely a transitive override; run `cargo tree -e features -i aws-lc-rs` to find the chain and adjust.

Also verify ring is still in the graph:
```bash
devenv shell -- bash -lc 'cargo tree -i ring | head -5'
```
Expected: `ring v0.17.x` rooted, with `rustls v0.23.x` directly above it.

- [ ] **Step 1.7: Build and test**

Run:
```bash
devenv shell -- bash -lc 'cargo build --workspace --tests'
```
Expected: SUCCESS, no compile errors.

```bash
devenv shell -- bash -lc 'cargo test --workspace --lib'
```
Expected: all unit tests pass. (Integration tests may need OpenShell — run those separately if available.)

- [ ] **Step 1.8: Functional smoke — actual TLS handshake**

Pick a binary code path that exercises reqwest TLS. Start the right binary and make sure it can hit `api.anthropic.com` (or whatever real HTTPS endpoint your local agent already talks to):

```bash
devenv shell -- bash -lc 'cargo run --bin right -- doctor 2>&1 | head -20'
```
Expected: `doctor` finishes without TLS-related panics. If `rustls panic: no process-level CryptoProvider available` appears, Step 1.5 did not take effect; re-check the placement of `install_default()`.

- [ ] **Step 1.9: Commit**

```bash
git add Cargo.toml crates/right/Cargo.toml crates/right/src/main.rs Cargo.lock
git commit -m "$(cat <<'EOF'
build(deps): drop aws-lc-rs, unify on ring crypto provider

reqwest 0.13's "rustls" feature pulled __rustls-aws-lc-rs, forcing
aws-lc-sys to compile (~2 min C build) on every clean checkout while
teloxide-core's reqwest 0.12 already brought ring. rustls 0.23 ended
up with both providers enabled in the graph.

Switch to reqwest's rustls-no-provider feature and install ring as the
process default in right's main(). Result: aws-lc-sys / aws-lc-rs drop
out of the graph entirely, rustls 0.23 compiles with the ring provider
only.
EOF
)"
```

**Note (from review):** the actual P0 commit (`42213564`) also stages
dev-dep `rustls` additions in `right-agent`, `right-bot`, `right-mcp`,
`right-memory`, `right-stt` plus a `setup_crypto()` helper in each.
These were required because tests bypass `main()` and need their own
provider install — see the P0 fixup commit `2ebab572` for the
consolidated form.

---

## Task 2: P1 — drop unused per-crate dependencies

**Files:**
- Modify: `crates/right-codegen/Cargo.toml`
- Modify: `crates/right-agent/Cargo.toml`
- Modify: `crates/right-openshell/Cargo.toml`

### Background

`cargo-machete --with-metadata` (run via `nix run nixpkgs#cargo-machete`) reports unused deps that survived the right-core decomposition. I spot-checked the findings with `rg` — the entries below have zero source references and are safe to drop. Entries that cargo-machete flags but are actually consumed via `tonic::include_proto!`-generated code (prost, prost-types, tonic-prost) stay and get an ignore entry in P3.

Each removal step is followed by a build to catch any cfg-gated or proc-macro usage cargo-machete missed.

### Steps

- [ ] **Step 2.1: Drop unused deps from `right-codegen`**

In `crates/right-codegen/Cargo.toml` `[dependencies]`, remove these three lines:
```toml
base64 = { workspace = true }
rand = { workspace = true }
tokio = { workspace = true }
```

- [ ] **Step 2.2: Verify right-codegen still compiles**

```bash
devenv shell -- bash -lc 'cargo build -p right-codegen --tests'
```
Expected: SUCCESS. If any of the three is reinstated by the compiler complaining "unresolved import", restore that single dep with a `# kept: <reason>` comment and continue.

- [ ] **Step 2.3: Drop unused deps from `right-agent`**

In `crates/right-agent/Cargo.toml` `[dependencies]` and `[dev-dependencies]`, remove these (delete the matching lines; preserve alphabetical order of the remaining lines):

`base64`, `fastrand`, `futures`, `hmac`, `http`, `http-body-util`, `hyper`, `hyper-util`, `include_dir`, `insta`, `minijinja`, `nix`, `owo-colors`, `prost`, `prost-types`, `rand`, `rmcp`, `rusqlite_migration`, `sha2`, `sse-stream`, `subtle`, `tokio-stream`, `tokio-util`, `tonic-prost`, `url`, `walkdir`.

- [ ] **Step 2.4: Verify right-agent still compiles**

```bash
devenv shell -- bash -lc 'cargo build -p right-agent --tests'
```
Expected: SUCCESS. If a build fails with "unresolved import" for one of the removed crates, restore *only that one* with a `# kept: <reason>` Cargo.toml comment and re-run.

- [ ] **Step 2.5: Drop verifiably-unused deps from `right-openshell`**

In `crates/right-openshell/Cargo.toml` `[dependencies]`, remove:
```toml
fs4 = { workspace = true }
http = { workspace = true }
hyper-util = { workspace = true, features = ["tokio"] }
```

Do **not** remove `prost`, `prost-types`, `tonic-prost`, or `tonic-prost-build` — those are consumed by the generated code from `tonic::include_proto!` and by `build.rs`. P3 records them in the cargo-machete ignore list.

- [ ] **Step 2.6: Verify right-openshell still compiles**

```bash
devenv shell -- bash -lc 'cargo build -p right-openshell --tests'
```
Expected: SUCCESS.

- [ ] **Step 2.7: Full workspace build for cross-crate safety**

```bash
devenv shell -- bash -lc 'cargo build --workspace --tests'
```
Expected: SUCCESS. If a crate outside the three above fails because it transitively expected one of the removed re-exports, restore the minimum needed dep there.

- [ ] **Step 2.8: Run clippy on the modified crates**

```bash
devenv shell -- bash -lc 'cargo clippy -p right-codegen -p right-agent -p right-openshell --tests -- -D warnings'
```
Expected: SUCCESS (no warnings).

- [ ] **Step 2.9: Commit**

```bash
git add crates/right-codegen/Cargo.toml crates/right-agent/Cargo.toml crates/right-openshell/Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
build(deps): prune unused per-crate dependencies

Post-right-core-decomposition leftovers identified by cargo-machete
and confirmed unused via grep:

right-codegen:  base64, rand, tokio
right-openshell: fs4, http, hyper-util
right-agent:    base64, fastrand, futures, hmac, http, http-body-util,
                hyper, hyper-util, include_dir, insta, minijinja, nix,
                owo-colors, prost, prost-types, rand, rmcp,
                rusqlite_migration, sha2, sse-stream, subtle,
                tokio-stream, tokio-util, tonic-prost, url, walkdir

Kept: prost/prost-types/tonic-prost in right-openshell — they are
consumed by code generated from tonic::include_proto!.
EOF
)"
```

---

## Task 3: P2 — tighten `tonic` features

**Files:**
- Modify: `Cargo.toml` (workspace dep `tonic`)

### Background

Today's entry:
```toml
tonic = { version = "0.14", features = ["tls-ring", "channel"] }
```
omits `default-features = false`, so the tonic 0.14 default set (which includes `codegen`, `prost`, `router`, `_tls-any`, plus some helpers) is unioned with our explicit additions. We use tonic only as a gRPC client/server for OpenShell — `channel` (client) and `tls-ring` (TLS provider). `codegen` is only needed by `tonic-prost-build` at build time, not by the `tonic` runtime crate. `router` is server-side only and we are a client (we use the OpenShell gRPC service, we don't serve gRPC ourselves — `right-openshell`'s `build_server(true)` setting generates server stubs but they're not used at runtime by us).

Verifying the exact features needed is the first step before touching anything.

### Steps

- [ ] **Step 3.1: Inspect tonic 0.14 features**

Run:
```bash
awk '/^\[features\]/{p=1} p' ~/.cargo/registry/src/index.crates.io-*/tonic-0.14.*/Cargo.toml 2>/dev/null | head -60
```
Expected: a list of `tonic` feature definitions with their internal flags. Skim it; confirm `channel` exists and is the client-side enabler, and `tls-ring` exists as a TLS provider feature. Note the `default = […]` line.

- [ ] **Step 3.2: Check whether `right-openshell` consumes tonic's server APIs**

Run:
```bash
rg -n 'tonic::transport::Server|tonic::server|build_server\(true\)' crates/right-openshell/ --type rust
```
Expected: hits in `build.rs` (it sets `build_server(true)` for codegen) and probably in test fixtures (`openshell_tests.rs` has a mock server). If a non-test runtime callsite uses `tonic::transport::Server`, the `router` feature must stay; otherwise drop it.

- [ ] **Step 3.3: Switch tonic to explicit-features**

In root `Cargo.toml`, replace:
```toml
tonic = { version = "0.14", features = ["tls-ring", "channel"] }
```
with:
```toml
tonic = { version = "0.14", default-features = false, features = ["transport", "tls-ring", "codegen", "router"] }
```

Notes for the engineer (verified against `tonic-0.14.6/Cargo.toml`):
- `transport = ["server", "channel"]` — covers both the client (`tonic::transport::Channel` used in `right-openshell/src/openshell.rs`) and `tonic::transport::Server::builder()` used by the mock in `right-openshell/src/openshell_tests.rs`.
- `codegen` adds `dep:async-trait` which is required for `#[tonic::async_trait]` (used in `openshell_tests.rs:123`).
- `tls-ring` enables `tokio-rustls/ring` — the rustls TLS provider already chosen project-wide.
- **Note (verified during execution):** `router` cannot be dropped. `tonic::transport::Server::builder().add_service(...)` is gated behind `router` in tonic 0.14 (the method exists only when `router` is enabled). The mock gRPC server in `right-openshell/src/openshell_tests.rs` uses it. Final feature list keeps `router`.

- [ ] **Step 3.4: Build and test**

```bash
devenv shell -- bash -lc 'cargo build --workspace --tests'
```
Expected: SUCCESS.

```bash
devenv shell -- bash -lc 'cargo test -p right-openshell --tests'
```
Expected: tests for the mock gRPC server still compile and pass.

- [ ] **Step 3.5: Confirm no feature regression**

```bash
devenv shell -- bash -lc 'cargo tree -e features -p tonic 2>&1 | head -20'
```
Expected: a tonic feature subtree containing `transport`, `channel` (pulled by `transport`), `server` (pulled by `transport`), `tls-ring`, `_tls-any` (pulled by `tls-ring`), `codegen`. **Not** present: `router` (we dropped it), `default` (we disabled it).

- [ ] **Step 3.6: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
build(deps): pin tonic to explicit feature set

tonic was using its default feature set unioned with our additions.
Set default-features = false and enumerate exactly what we use:
channel + transport for the client, router for the mock server in
right-openshell's integration tests, tls-ring for TLS.
EOF
)"
```

---

## Task 4: P3 — make `cargo-machete` re-runnable cleanly

**Files:**
- Modify: `crates/right-openshell/Cargo.toml` (add `[package.metadata.cargo-machete]`)

### Background

After P1, `cargo-machete` will still report `prost`, `prost-types`, and `tonic-prost` as unused in `right-openshell` because it doesn't scan code generated by `tonic::include_proto!` macros. We need these deps at runtime — the generated code uses fully-qualified `::prost::Message` etc. Add a `[package.metadata.cargo-machete]` ignore so future audits don't surface this noise.

### Steps

- [ ] **Step 4.1: Add the ignore section**

At the bottom of `crates/right-openshell/Cargo.toml`, append:

```toml

[package.metadata.cargo-machete]
ignored = ["prost", "prost-types", "tonic-prost"]
# These are used via fully-qualified paths inside code generated by
# tonic::include_proto! in src/lib.rs. cargo-machete cannot see them
# because it does not expand macros.
```

- [ ] **Step 4.2: Verify cargo-machete is now clean**

```bash
devenv shell -- bash -lc 'nix run nixpkgs#cargo-machete -- --with-metadata 2>&1 | tail -30'
```
Expected: no `right-codegen`, `right-openshell`, or `right-agent` entries in the unused-deps section (since P1 emptied them and P3 quiets the false positives). The command should end with `Done!`.

If cargo-machete still reports an entry, either (a) the previous task left a dep behind — investigate and remove — or (b) the false-positive list is incomplete; add the entry to `ignored` with a one-line comment explaining why.

- [ ] **Step 4.3: Commit**

```bash
git add crates/right-openshell/Cargo.toml
git commit -m "$(cat <<'EOF'
build(deps): silence cargo-machete false positives for tonic prost codegen

prost/prost-types/tonic-prost are consumed by code generated from
tonic::include_proto! in right-openshell::lib.rs. cargo-machete can
not see macro-expanded usage, so it reports them as unused. Document
the exception so `nix run nixpkgs#cargo-machete -- --with-metadata`
returns clean.
EOF
)"
```

---

## Task 5: post-flight verification

- [ ] **Step 5.1: Measure cold-build delta**

```bash
devenv shell -- bash -lc 'cargo clean -p reqwest -p rustls -p hyper-rustls -p tokio-rustls -p rustls-platform-verifier -p ring && time cargo check --workspace --tests' 2>&1 | tail -5 | tee /tmp/after-time.txt
diff /tmp/before-time.txt /tmp/after-time.txt
```
Expected: the `real` line in `/tmp/after-time.txt` should be at least ~60 seconds faster than `/tmp/before-time.txt` (dropping aws-lc-sys's C build dominates the saving). The actual number depends on hardware; the qualitative drop is what matters.

- [ ] **Step 5.2: Full workspace clippy + test**

```bash
devenv shell -- bash -lc 'cargo clippy --workspace --tests -- -D warnings && cargo test --workspace'
```
Expected: SUCCESS on both. Any failure here is a regression introduced by P0-P3 and must be fixed before merging.

- [ ] **Step 5.3: Push the branch and open a PR**

Branch name: `dep-audit`. After verifying locally:

```bash
git push -u origin dep-audit
gh pr create --title "Dependency audit cleanup (P0-P3)" --body "$(cat <<'EOF'
## Summary
- P0: drop aws-lc-rs crypto backend; install ring as rustls default provider
- P1: prune unused per-crate deps left over from right-core decomposition
- P2: pin tonic to explicit feature set
- P3: silence cargo-machete false positives for tonic-generated code

## Test plan
- [ ] `cargo clippy --workspace --tests -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo tree -i aws-lc-sys` returns "not found"
- [ ] `nix run nixpkgs#cargo-machete -- --with-metadata` returns clean
- [ ] Manual: `cargo run --bin right -- doctor` completes (TLS round-trip via reqwest works)
- [ ] Manual: bring up `right up` against a local OpenShell, exercise voice STT once (whisper-rs path still works)
EOF
)"
```

---

## Rollback

Each task is one commit. To revert any single P-level change:

```bash
git revert <commit-sha>     # creates a revert commit on top
# or, if pre-push: git reset --hard HEAD~1
```

The four commits are independent; reverting P0 does not require reverting P1-P3 (and vice versa).
