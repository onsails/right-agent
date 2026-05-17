//! Integration test for `claude upgrade` inside an OpenShell sandbox.
//!
//! Creates an ephemeral sandbox via `right_openshell::test_support::TestSandbox`,
//! runs `claude install` plus `claude upgrade`, and asserts the full
//! post-upgrade state.
//! CI-explicit ignored test because it requires a live OpenShell gateway and
//! runs real `claude upgrade` over the network.
//! If Claude's download service denies a GitHub runner with 403, the test logs
//! the denial and exits early; production handles that path by logging the
//! failed startup upgrade attempt and leaving the baked npm Claude in place.

use right_openshell::test_support::TestSandbox;

/// Full lifecycle: install registers the native target, upgrade runs, symlink
/// appears, upgraded binary reports a Claude Code version, and PATH precedence
/// favours `/sandbox/.local/bin`.
///
/// `#[ignore]` exception (against the project's general no-ignore rule for
/// integration tests): this test runs a real `claude upgrade` over the
/// network — 30–60s — which dominates the workspace test suite. Run on
/// demand with `cargo test -p right-bot --test sandbox_upgrade -- --ignored`.
/// TODO: replace with a per-host binary cache so the steady-state path hits
/// the idempotent "Current version" branch in seconds and `#[ignore]` can
/// be removed.
#[ignore = "ci-claude: runs real claude upgrade inside live OpenShell sandbox"]
#[tokio::test]
async fn ci_claude_upgrade_lifecycle() {
    let sbox = TestSandbox::create("claude-upgrade").await;

    // 1. Match production startup: register native install metadata before
    //    upgrade. Without this, Claude may only repair config and leave the
    //    native symlink absent.
    let (stdout, exit) = sbox.exec_with_timeout(&["claude", "install"], 180).await;
    if claude_install_download_denied(exit, &stdout) {
        eprintln!("skipping claude upgrade lifecycle: Claude download service denied this runner");
        return;
    }
    assert_eq!(exit, 0, "claude install failed: {stdout}");

    // 2. `claude upgrade` reports either a fresh install or the idempotent
    //    current-version path. Claude currently exits 1 for "up to date", so
    //    treat that specific output as a successful no-op.
    let (stdout, exit) = sbox.exec_with_timeout(&["claude", "upgrade"], 180).await;
    assert!(
        claude_upgrade_success(exit, &stdout),
        "claude upgrade failed with exit {exit}; stdout: {stdout}"
    );
    assert!(
        stdout.contains("Successfully updated") || claude_upgrade_up_to_date(&stdout),
        "unexpected upgrade output: {stdout}"
    );

    // 3. The symlink `/sandbox/.local/bin/claude` now exists.
    let (_, exit) = sbox
        .exec(&["test", "-L", "/sandbox/.local/bin/claude"])
        .await;
    assert_eq!(exit, 0, "/sandbox/.local/bin/claude symlink missing");

    // 4. The upgraded binary runs and reports a Claude Code version.
    let (stdout, exit) = sbox
        .exec(&["/sandbox/.local/bin/claude", "--version"])
        .await;
    assert_eq!(exit, 0, "upgraded binary failed to run");
    assert!(
        stdout.contains("Claude Code"),
        "expected 'Claude Code' in version output, got: {stdout}"
    );

    // 5. PATH precedence: with `/sandbox/.local/bin` prepended, `which claude`
    //    resolves to the upgraded path, not the image's `/usr/local/bin/claude`.
    let (stdout, exit) = sbox
        .exec(&["bash", "-c", "PATH=/sandbox/.local/bin:$PATH which claude"])
        .await;
    assert_eq!(exit, 0, "`which claude` failed: {stdout}");
    assert_eq!(
        stdout.trim(),
        "/sandbox/.local/bin/claude",
        "expected /sandbox/.local/bin/claude, got: {stdout}"
    );
}

fn claude_upgrade_success(exit: i32, stdout: &str) -> bool {
    exit == 0 || (exit == 1 && claude_upgrade_up_to_date(stdout))
}

fn claude_upgrade_up_to_date(stdout: &str) -> bool {
    stdout.contains("Current version")
}

fn claude_install_download_denied(exit: i32, stdout: &str) -> bool {
    exit != 0
        && stdout.contains("downloads.claude.ai/claude-code-releases/latest")
        && stdout.contains("status code 403")
}

#[test]
fn claude_install_download_denied_detects_runner_403() {
    let stdout = "\
Checking installation status...
Installing Claude Code native build latest...
Failed to fetch version from https://downloads.claude.ai/claude-code-releases/latest: \
Request failed with status code 403";

    assert!(claude_install_download_denied(1, stdout));
}

#[test]
fn claude_install_download_denied_rejects_other_failures() {
    assert!(!claude_install_download_denied(
        1,
        "Failed to fetch version from https://downloads.claude.ai/claude-code-releases/latest: timeout"
    ));
    assert!(!claude_install_download_denied(
        0,
        "Request failed with status code 403"
    ));
}
