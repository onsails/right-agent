//! Integration test for `claude upgrade` inside an OpenShell sandbox.
//!
//! Creates an ephemeral sandbox via `rightclaw::test_support::TestSandbox`,
//! runs `claude upgrade`, and asserts the full post-upgrade state.
//! Requires a running OpenShell gateway (dev machines have it — no #[ignore]).

use rightclaw::test_support::TestSandbox;

/// Full lifecycle: upgrade runs, symlink appears, upgraded binary reports
/// a Claude Code version, and PATH precedence favours `/sandbox/.local/bin`.
#[tokio::test]
async fn claude_upgrade_lifecycle() {
    let sbox = TestSandbox::create("claude-upgrade").await;

    // 1. `claude upgrade` exits 0 and reports either a fresh install or
    //    "Current version" (idempotent re-run). 180s — fetch + install over
    //    the network doesn't fit the 10s default.
    let (stdout, exit) = sbox.exec_with_timeout(&["claude", "upgrade"], 180).await;
    assert_eq!(exit, 0, "claude upgrade failed; stdout: {stdout}");
    assert!(
        stdout.contains("Successfully updated") || stdout.contains("Current version"),
        "unexpected upgrade output: {stdout}"
    );

    // 2. The symlink `/sandbox/.local/bin/claude` now exists.
    let (_, exit) = sbox
        .exec(&["test", "-L", "/sandbox/.local/bin/claude"])
        .await;
    assert_eq!(exit, 0, "/sandbox/.local/bin/claude symlink missing");

    // 3. The upgraded binary runs and reports a Claude Code version.
    let (stdout, exit) = sbox
        .exec(&["/sandbox/.local/bin/claude", "--version"])
        .await;
    assert_eq!(exit, 0, "upgraded binary failed to run");
    assert!(
        stdout.contains("Claude Code"),
        "expected 'Claude Code' in version output, got: {stdout}"
    );

    // 4. PATH precedence: with `/sandbox/.local/bin` prepended, `which claude`
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
