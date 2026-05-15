# Test Suite Split And Speedups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep ordinary local and PR test runs deterministic and reasonably fast, move live OpenShell, Claude, and real STT coverage behind explicit ignored tests, and add GitHub Actions jobs that install the required external tools and call those ignored tests deliberately.

**Architecture:** Split tests by dependency boundary. Pure Rust, local filesystem, mock HTTP, and mock OpenShell-gRPC tests remain in the default `cargo test` path. Tests that require a live OpenShell gateway, a sandbox image with Claude Code, OpenShell file transfer, or real ffmpeg/Whisper inference become `#[ignore]` with stable `ci-*` reasons and are invoked by named workflow jobs. Slow tests that do not truly require external services are rewritten, not ignored.

**Tech Stack:** Rust 2024, Cargo/libtest `--test-threads=1`, GitHub Actions, `devenv shell` for the Rust/toolchain environment, direct NVIDIA OpenShell installer, rootless Podman socket for the OpenShell gateway, ffmpeg, libclang for bindgen, Whisper tiny model cache, existing `devenv.nix`.

**Timing source:** `/tmp/rightclaw-test-timing-stats.md` from the serial run on 2026-05-15.

---

## Non-negotiables

- Do not install OpenShell on the local machine as part of this work. Only GitHub workflow files get OpenShell installation steps.
- Do not use the project `install.sh` in CI for OpenShell setup. It installs Right Agent, then runs interactive `right init`, which can block a workflow.
- Use the direct OpenShell installer in CI:

File: `.github/workflows/tests.yml`

```yaml
- name: Install OpenShell
  run: |
    secret="$(openssl rand -hex 32)"
    echo "OPENSHELL_SSH_HANDSHAKE_SECRET=$secret" >> "$GITHUB_ENV"
    systemctl --user set-environment OPENSHELL_SSH_HANDSHAKE_SECRET="$secret"
    curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | OPENSHELL_SSH_HANDSHAKE_SECRET="$secret" sh
    openshell --help
```

- Keep normal Cargo compilation parallel. Limit runtime test scheduling with libtest flags:

File: `.github/workflows/tests.yml`

```yaml
run: devenv shell -- cargo test --workspace --lib --bins --tests --no-fail-fast --locked -- --test-threads=1
```

- Exclude doc tests from the measured/default workflow path for this cleanup. The timing run was explicitly finalized without doc tests.
- Final local verification after code changes still uses the workspace test path with one runtime thread:

```sh
devenv shell -- cargo test --workspace --lib --bins --tests --no-fail-fast -- --test-threads=1
```

---

## Task 1: Update Test Policy Documentation

**Files:**
- Modify: `AGENTS.rust.md`
- Modify: `ARCHITECTURE.md`
- Modify if drifted: `docs/architecture/sandbox.md`

- [ ] Replace the current blanket rule in `AGENTS.rust.md` that forbids `#[ignore]` on OpenShell integration tests.

File: `AGENTS.rust.md`

```md
- **External integration tests**: tests requiring live OpenShell, Claude Code inside a sandbox, real ffmpeg/Whisper inference, or network model downloads must be `#[ignore]` locally and explicitly invoked by GitHub Actions jobs. Use stable ignore reasons:
  - `ci-openshell: ...`
  - `ci-claude: ...`
  - `ci-stt: ...`
```

- [ ] Add a short architecture note that `right-openshell` owns live sandbox helpers, but default workspace tests must not require a live gateway.

File: `ARCHITECTURE.md`

```md
Live OpenShell coverage is CI-explicit: tests that create real sandboxes or rely on OpenShell CLI file transfer use `#[ignore = "ci-openshell: ..."]` and are called by `.github/workflows/tests.yml`. Mock gRPC and pure policy tests remain in the default workspace test path.
```

- [ ] Re-read `docs/architecture/sandbox.md` after the code edits. Update only if the test split changes the documented sandbox lifecycle or live-test support behavior.

**Verification:**

```sh
devenv shell -- cargo test --workspace --lib --bins --tests --no-fail-fast -- --test-threads=1
```

Expected: default workspace tests do not try to create OpenShell sandboxes.

---

## Task 2: Remove The Unnecessary Sandbox Grep Test

**Files:**
- Modify: `crates/bot/tests/cc_debug_integration.rs`

- [ ] Delete `skill_can_grep_jsonl`. It only proves that `grep` can find a marker in a file inside `/sandbox`; that is not a Right Agent contract and does not justify a live sandbox.

- [ ] Keep these tests, but mark them as Claude/OpenShell CI-only:

File: `crates/bot/tests/cc_debug_integration.rs`

```rust
#[ignore = "ci-claude: requires live OpenShell sandbox and claude binary"]
#[tokio::test]
async fn cc_debug_file_lands_inside_sandbox() {
```

File: `crates/bot/tests/cc_debug_integration.rs`

```rust
#[ignore = "ci-claude: requires live OpenShell sandbox and claude binary"]
#[tokio::test]
async fn jsonl_project_dir_is_accessible_and_cc_preserves_contents() {
```

- [ ] Update the file header from "no `#[ignore]`" to "CI-explicit ignored tests".

**Verification:**

```sh
devenv shell -- cargo test -p right-bot --test cc_debug_integration -- --test-threads=1
```

Expected: 0 passed, 0 failed, 2 ignored.

```sh
devenv shell -- cargo test -p right-bot --test cc_debug_integration -- --ignored --test-threads=1
```

Expected on a machine with OpenShell and Claude: both retained tests pass.

---

## Task 3: Move Live OpenShell Tests Behind Explicit Ignores

**Files:**
- Modify: `crates/right-openshell/src/openshell_tests.rs`
- Modify: `crates/right-agent/tests/control_master.rs`
- Modify: `crates/right-agent/tests/policy_apply.rs`
- Modify: `crates/right-agent/tests/rebootstrap_sandbox.rs`
- Modify: `crates/right/tests/cli_integration.rs`
- Modify: `crates/bot/tests/sandbox_upgrade.rs`

- [ ] Add `#[ignore = "ci-openshell: requires live OpenShell gateway"]` to live OpenShell tests that create or mutate a sandbox:

File: `crates/right-agent/tests/control_master.rs`

```rust
#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn control_master_engages_after_first_ssh_call() {
```

File: `crates/right-agent/tests/rebootstrap_sandbox.rs`

```rust
#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn execute_against_live_sandbox() {
```

File: `crates/right/tests/cli_integration.rs`

```rust
#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn test_policy_validates_against_openshell() {
```

- [ ] Add the same ignore reason to the live tests in `crates/right-agent/tests/policy_apply.rs`.

- [ ] Change `crates/bot/tests/sandbox_upgrade.rs` from its current ad hoc slow ignore reason to:

File: `crates/bot/tests/sandbox_upgrade.rs`

```rust
#[ignore = "ci-claude: runs real claude upgrade inside live OpenShell sandbox"]
#[tokio::test]
async fn claude_upgrade_lifecycle() {
```

- [ ] In `crates/right-openshell/src/openshell_tests.rs`, mark the remaining live section tests as `ci-openshell` after Task 4 removes or rewrites the redundant transfer variants.

**Verification:**

```sh
devenv shell -- cargo test -p right-agent --test control_master -- --test-threads=1
devenv shell -- cargo test -p right-agent --test policy_apply -- --test-threads=1
devenv shell -- cargo test -p right-agent --test rebootstrap_sandbox -- --test-threads=1
devenv shell -- cargo test -p right --test cli_integration test_policy_validates_against_openshell -- --test-threads=1
```

Expected: each command reports the live tests as ignored, not failed.

---

## Task 4: Reduce OpenShell Upload/Download Live Coverage

**Files:**
- Modify: `crates/right-openshell/src/openshell.rs`
- Modify: `crates/right-openshell/src/openshell_tests.rs`

- [ ] Keep these live tests, but mark them ignored:
  - `verify_sandbox_files_detects_missing_and_reuploads`
  - `exec_immediately_after_sandbox_create_reproduces_init_flow`
  - `verify_sandbox_files_passes_when_all_present`
  - `upload_file_to_directory`
  - `upload_directory_preserves_files_and_overwrites`
  - `download_file_writes_to_exact_dest_path`

File: `crates/right-openshell/src/openshell_tests.rs`

```rust
#[ignore = "ci-openshell: requires live OpenShell gateway"]
#[tokio::test]
async fn upload_file_to_directory() {
```

- [ ] Expand `upload_directory_preserves_files_and_overwrites` to include a nested file, then remove the separate `upload_file_to_nested_dir` live test.

File: `crates/right-openshell/src/openshell_tests.rs`

```rust
std::fs::create_dir_all(skill_dir.join("nested")).unwrap();
std::fs::write(skill_dir.join("nested/TOOL.md"), "nested\n").unwrap();
```

- [ ] Remove `upload_file_overwrites_existing`. The retained directory upload test already checks overwrite, and the single-file upload path is covered by `upload_file_to_directory`.

- [ ] Rewrite these live download tests into local unit tests over extracted helpers:
  - `download_file_overwrites_existing_file`
  - `download_file_replaces_stale_directory_at_dest`
  - `download_file_creates_parent_directory`

- [ ] Extract local helper(s) from `download_file` so path behavior can be tested without OpenShell:

File: `crates/right-openshell/src/openshell.rs`

```rust
fn ensure_download_parent(host_dest: &Path) -> miette::Result<()> {
    let parent = host_dest.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| {
        miette::miette!(
            "failed to create parent directory {}: {e:#}",
            parent.display()
        )
    })
}

fn remove_stale_directory_at_dest(host_dest: &Path) -> miette::Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(host_dest)
        && meta.is_dir()
    {
        std::fs::remove_dir_all(host_dest).map_err(|e| {
            miette::miette!(
                "failed to remove stale directory at {}: {e:#}",
                host_dest.display()
            )
        })?;
    }
    Ok(())
}
```

- [ ] Use those helpers from `download_file` before the final rename.

- [ ] Add local tests for parent creation, stale directory removal, and overwrite-by-rename. These tests must not call `shared_test_sandbox()`.

**Verification:**

```sh
devenv shell -- cargo test -p right-openshell -- --test-threads=1
```

Expected: local helper tests pass; live OpenShell tests are ignored by default.

```sh
devenv shell -- cargo test -p right-openshell upload_directory_preserves_files_and_overwrites -- --ignored --test-threads=1
devenv shell -- cargo test -p right-openshell download_file_writes_to_exact_dest_path -- --ignored --test-threads=1
```

Expected on a machine with OpenShell: retained live transfer coverage passes.

---

## Task 5: Move Real STT To CI, Keep Cheap STT Local

**Files:**
- Modify: `crates/bot/src/stt/mod.rs`
- Modify: `crates/bot/src/stt/whisper.rs`
- Modify: `devenv.nix`

- [ ] Add `ffmpeg` and a Nix `LIBCLANG_PATH` to `devenv.nix` so local and CI shells have the same STT and bindgen prerequisites when using devenv.

File: `devenv.nix`

```nix
    ffmpeg

  env.LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
```

- [ ] Mark real inference tests ignored:

File: `crates/bot/src/stt/mod.rs`

```rust
#[ignore = "ci-stt: runs real ffmpeg and Whisper inference"]
#[tokio::test]
async fn voice_success_returns_success_marker() {
```

File: `crates/bot/src/stt/mod.rs`

```rust
#[ignore = "ci-stt: runs real ffmpeg and Whisper inference"]
#[tokio::test]
async fn transcribe_voice_end_to_end() {
```

File: `crates/bot/src/stt/mod.rs`

```rust
#[ignore = "ci-stt: runs real ffmpeg and Whisper inference"]
#[tokio::test]
async fn transcribe_video_note_end_to_end() {
```

File: `crates/bot/src/stt/whisper.rs`

```rust
#[ignore = "ci-stt: runs real ffmpeg and Whisper inference"]
#[tokio::test]
async fn inference_returns_known_words() {
```

- [ ] Rewrite `ffmpeg_unavailable_returns_error_marker_without_running_ffmpeg` so it does not call `tiny_ctx(false)` and does not download the tiny model.

File: `crates/bot/src/stt/mod.rs`

```rust
let ctx = SttContext {
    transcriber: Transcriber::new(PathBuf::from("/nonexistent/not-used.bin")),
    ffmpeg_available: false,
};
```

**Verification:**

```sh
devenv shell -- cargo test -p right-bot stt::ffmpeg_unavailable_returns_error_marker_without_running_ffmpeg -- --test-threads=1
devenv shell -- cargo test -p right-bot stt:: -- --test-threads=1
```

Expected: cheap STT tests pass locally; real inference tests are ignored.

```sh
devenv shell -- cargo test -p right-bot stt:: -- --ignored --test-threads=1
```

Expected with ffmpeg and cached/downloadable tiny model: real STT tests pass.

---

## Task 6: Rewrite Slow Tests That Do Not Need CI-Only Treatment

**Files:**
- Modify: `crates/right-agent/tests/memory_failure_scenarios.rs`
- Modify: `crates/right-memory/src/hindsight.rs`
- Modify: `crates/right-stt/src/lib.rs`
- Modify: `crates/right/tests/cli_integration.rs`

### 6A: Replace The 31s Breaker Sleep With Paused Tokio Time

- [ ] Rewrite `recovery_drains_queue_after_breaker_closes` to use paused time.

File: `crates/right-agent/tests/memory_failure_scenarios.rs`

```rust
#[tokio::test(start_paused = true)]
async fn recovery_drains_queue_after_breaker_closes() {
```

File: `crates/right-agent/tests/memory_failure_scenarios.rs`

```rust
tokio::time::advance(std::time::Duration::from_secs(31)).await;
tokio::task::yield_now().await;
```

- [ ] Remove the real `tokio::time::sleep(Duration::from_secs(31))`.

**Verification:**

```sh
devenv shell -- cargo test -p right-agent --test memory_failure_scenarios recovery_drains_queue_after_breaker_closes -- --test-threads=1
```

Expected: completes in under 3 seconds.

### 6B: Make Hindsight Retain Timeout Injectable In Tests

- [ ] Add a per-client retain timeout field with the current production default.

File: `crates/right-memory/src/hindsight.rs`

```rust
pub struct HindsightClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    bank_id: String,
    budget: String,
    max_tokens: u32,
    retain_timeout: Duration,
}
```

File: `crates/right-memory/src/hindsight.rs`

```rust
retain_timeout: RETAIN_TIMEOUT,
```

- [ ] Add a test-only builder:

File: `crates/right-memory/src/hindsight.rs`

```rust
#[cfg(test)]
pub(crate) fn with_retain_timeout(mut self, timeout: Duration) -> Self {
    self.retain_timeout = timeout;
    self
}
```

- [ ] Replace `.timeout(RETAIN_TIMEOUT)` with `.timeout(self.retain_timeout)`.

- [ ] Change `retain_timeout_maps_to_timeout_variant` and `retain_json_body_timeout_maps_to_timeout_variant` to use `with_retain_timeout(Duration::from_millis(100))`; keep server stalls longer than that but short enough not to waste wall clock.

**Verification:**

```sh
devenv shell -- cargo test -p right-memory hindsight::tests::retain_timeout_maps_to_timeout_variant -- --test-threads=1
devenv shell -- cargo test -p right-memory hindsight::tests::retain_json_body_timeout_maps_to_timeout_variant -- --test-threads=1
```

Expected: both complete in under 2 seconds total.

### 6C: Replace httpbin With A Local 404 Server

- [ ] Rewrite `download_url_to_path_bad_status_returns_bad_status_error` to bind a local `tokio::net::TcpListener` and return a fixed HTTP 404.

File: `crates/right-stt/src/lib.rs`

```rust
let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
let url = format!("http://{}", listener.local_addr().unwrap());
let server = tokio::spawn(async move {
    let (mut stream, _) = listener.accept().await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf).await;
    stream
        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
});
let result = download_url_to_path(&url, "test", &dest).await;
server.await.unwrap();
```

- [ ] Assert only `DownloadError::BadStatus { status: 404, .. }`. Remove the current network-unavailable branch.

**Verification:**

```sh
devenv shell -- cargo test -p right-stt download_url_to_path_bad_status_returns_bad_status_error -- --test-threads=1
```

Expected: offline deterministic pass.

### 6D: Collapse Redundant CLI Init Subprocess Tests

- [ ] Keep CLI coverage, but stop running the full interactive `right init` path for every small assertion.

- [ ] Combine these tests into one init smoke test:
  - `test_init_creates_structure`
  - `test_init_generates_per_agent_codegen`
  - `test_list_after_init`
  - `test_doctor_in_valid_home`

- [ ] Keep negative and distinct behavior tests separate:
  - `test_init_twice_fails`
  - `test_init_with_telegram_token`
  - `test_init_with_invalid_telegram_token`
  - agent init/backup/restore/destroy tests

- [ ] Where possible, replace CLI subprocess setup with direct fixture files plus `write_minimal_tunnel_config(home)` for commands that only need an existing home.

**Verification:**

```sh
devenv shell -- cargo test -p right --test cli_integration -- --test-threads=1
```

Expected: `cli_integration` wall time drops from roughly 33.6s to materially lower without reducing distinct CLI behavior coverage.

---

## Task 7: Add Explicit GitHub Workflows

**Files:**
- Create: `.github/workflows/tests.yml`

- [ ] Add the default workspace job. It does not run ignored tests and does not run doc tests.

File: `.github/workflows/tests.yml`

```yaml
name: Tests

on:
  pull_request:
  push:
    branches: ["master"]
  workflow_dispatch:

env:
  CARGO_TERM_COLOR: always

jobs:
  workspace:
    name: workspace tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: cachix/install-nix-action@v31
      - uses: cachix/cachix-action@v16
        with:
          name: devenv
      - name: Install devenv.sh
        run: nix profile add nixpkgs#devenv
      - name: Provide external-tool test stubs
        run: |
          mkdir -p "$RUNNER_TEMP/bin"
          cat > "$RUNNER_TEMP/bin/cloudflared" <<'SH'
          #!/bin/sh
          echo "cloudflared test stub" >&2
          exit 0
          SH
          cat > "$RUNNER_TEMP/bin/claude" <<'SH'
          #!/bin/sh
          echo "claude test stub" >&2
          exit 0
          SH
          chmod +x "$RUNNER_TEMP/bin/cloudflared" "$RUNNER_TEMP/bin/claude"
          echo "$RUNNER_TEMP/bin" >> "$GITHUB_PATH"
      - name: Test workspace serially
        run: |
          devenv shell -- bash -lc "export PATH=\"\$RUNNER_TEMP/bin:\$PATH\"; cargo test --workspace --lib --bins --tests --no-fail-fast --locked -- --test-threads=1"
```

- [ ] Add STT CI job. It runs inside `devenv shell`, caches the tiny Whisper model, and runs ignored STT tests explicitly.

File: `.github/workflows/tests.yml`

```yaml
  stt:
    name: stt ignored tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: cachix/install-nix-action@v31
      - uses: cachix/cachix-action@v16
        with:
          name: devenv
      - name: Install devenv.sh
        run: nix profile add nixpkgs#devenv
      - uses: actions/cache@v4
        with:
          path: ~/.right/cache/whisper
          key: whisper-ggml-tiny-v1
      - name: Run STT ignored tests serially
        run: |
          devenv shell -- cargo test --workspace --no-fail-fast --locked ci_stt -- --ignored --test-threads=1
```

- [ ] Add OpenShell CI job. It enters `devenv shell` for Rust tooling, installs OpenShell in the workflow, starts the Podman socket and gateway, waits for mTLS certs, then runs ignored OpenShell tests explicitly.

File: `.github/workflows/tests.yml`

```yaml
  openshell:
    name: openshell ignored tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: cachix/install-nix-action@v31
      - uses: cachix/cachix-action@v16
        with:
          name: devenv
      - name: Install devenv.sh
        run: nix profile add nixpkgs#devenv
      - name: Install system deps
        run: sudo apt-get update && sudo apt-get install -y podman
      - name: Start Podman socket
        run: |
          systemctl --user enable --now podman.socket
          for attempt in $(seq 1 30); do
            if test -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock"; then
              podman info
              exit 0
            fi
            echo "waiting for Podman socket ($attempt/30)"
            sleep 1
          done
          systemctl --user status podman.socket --no-pager || true
          journalctl --user -u podman.socket --no-pager -n 80 || true
          exit 1
      - name: Install OpenShell
        run: |
          secret="$(openssl rand -hex 32)"
          echo "OPENSHELL_SSH_HANDSHAKE_SECRET=$secret" >> "$GITHUB_ENV"
          systemctl --user set-environment OPENSHELL_SSH_HANDSHAKE_SECRET="$secret"
          curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | OPENSHELL_SSH_HANDSHAKE_SECRET="$secret" sh
          openshell --help
      - name: Wait for OpenShell gateway
        run: |
          for attempt in $(seq 1 90); do
            if openshell status >/dev/null 2>&1 \
              && test -f "$HOME/.config/openshell/gateways/openshell/mtls/ca.crt" \
              && test -f "$HOME/.config/openshell/gateways/openshell/mtls/tls.crt" \
              && test -f "$HOME/.config/openshell/gateways/openshell/mtls/tls.key"; then
              openshell status
              exit 0
            fi
            echo "waiting for OpenShell gateway ($attempt/90)"
            sleep 2
          done
          systemctl --user status openshell-gateway --no-pager || true
          journalctl --user -u openshell-gateway --no-pager -n 80 || true
          exit 1
      - name: OpenShell doctor
        run: openshell doctor check
      - name: Run OpenShell ignored tests serially
        run: |
          devenv shell -- bash -lc "export PATH=\"\$PATH:/usr/local/bin:/usr/bin\"; cargo test --workspace --no-fail-fast --locked ci_openshell -- --ignored --test-threads=1"
```

- [ ] Add Claude/OpenShell CI job separately so `claude upgrade` failures are isolated from OpenShell-only regressions.

File: `.github/workflows/tests.yml`

```yaml
  claude-openshell:
    name: claude ignored tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: cachix/install-nix-action@v31
      - uses: cachix/cachix-action@v16
        with:
          name: devenv
      - name: Install devenv.sh
        run: nix profile add nixpkgs#devenv
      - name: Install system deps
        run: sudo apt-get update && sudo apt-get install -y podman
      - name: Start Podman socket
        run: |
          systemctl --user enable --now podman.socket
          for attempt in $(seq 1 30); do
            if test -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock"; then
              podman info
              exit 0
            fi
            echo "waiting for Podman socket ($attempt/30)"
            sleep 1
          done
          systemctl --user status podman.socket --no-pager || true
          journalctl --user -u podman.socket --no-pager -n 80 || true
          exit 1
      - name: Install OpenShell
        run: |
          secret="$(openssl rand -hex 32)"
          echo "OPENSHELL_SSH_HANDSHAKE_SECRET=$secret" >> "$GITHUB_ENV"
          systemctl --user set-environment OPENSHELL_SSH_HANDSHAKE_SECRET="$secret"
          curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | OPENSHELL_SSH_HANDSHAKE_SECRET="$secret" sh
          openshell --help
      - name: Wait for OpenShell gateway
        run: |
          for attempt in $(seq 1 90); do
            if openshell status >/dev/null 2>&1 \
              && test -f "$HOME/.config/openshell/gateways/openshell/mtls/ca.crt" \
              && test -f "$HOME/.config/openshell/gateways/openshell/mtls/tls.crt" \
              && test -f "$HOME/.config/openshell/gateways/openshell/mtls/tls.key"; then
              openshell status
              exit 0
            fi
            echo "waiting for OpenShell gateway ($attempt/90)"
            sleep 2
          done
          systemctl --user status openshell-gateway --no-pager || true
          journalctl --user -u openshell-gateway --no-pager -n 80 || true
          exit 1
      - name: OpenShell doctor
        run: openshell doctor check
      - name: Run Claude/OpenShell ignored tests serially
        run: |
          devenv shell -- bash -lc "export PATH=\"\$PATH:/usr/local/bin:/usr/bin\"; cargo test --workspace --no-fail-fast --locked ci_claude -- --ignored --test-threads=1"
```

- [ ] If `claude-openshell` flakes because the CI sandbox image lacks the Claude binary, do not silently skip the job. Add a test helper that installs or upgrades Claude inside the sandbox with `claude upgrade` and keep the failure visible.

**Verification:**

```sh
devenv shell -- actionlint .github/workflows/tests.yml
```

Expected: no workflow syntax errors.

---

## Task 8: Final Timing And Regression Check

**Files:**
- Update: `/tmp/rightclaw-test-timing-stats-after.md`

- [ ] Run the default serial workspace test command without doc tests:

```sh
time devenv shell -- cargo test --workspace --lib --bins --tests --no-fail-fast -- --test-threads=1
```

- [ ] Write the new timing report to `/tmp/rightclaw-test-timing-stats-after.md` with:
  - total wall time
  - compile time if visible
  - passed/failed/ignored counts
  - slowest remaining targets
  - list of tests moved to ignored CI jobs
  - list of tests removed or rewritten

- [ ] Run targeted ignored suites on a machine with the external prerequisites:

```sh
devenv shell -- cargo test --workspace --no-fail-fast --locked ci_openshell -- --ignored --test-threads=1
devenv shell -- cargo test --workspace --no-fail-fast --locked ci_stt -- --ignored --test-threads=1
devenv shell -- cargo test --workspace --no-fail-fast --locked ci_claude -- --ignored --test-threads=1
```

- [ ] Do not claim GitHub Actions coverage is complete until the workflow has run at least once or `act`/`actionlint` plus local command parity has been checked.

**Expected outcome:**

- Default local test run no longer starts OpenShell sandboxes.
- Default local test run no longer downloads Whisper models or runs real inference.
- No local test depends on `httpbin.org`.
- The 31s breaker test and 10s/12s Hindsight timeout tests complete in seconds.
- CI has explicit jobs for `ci-openshell`, `ci-claude`, and `ci-stt` ignored tests.
- OpenShell installation exists in the workflow, not as a local machine setup step.
