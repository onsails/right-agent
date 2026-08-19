//! Live-microVM cover for the guest side of `right agent migrate-sandbox`:
//! the archive an OpenShell sandbox produces really does unpack into a fresh
//! Agent Sandbox, and the verification that gates the destructive step really
//! does fail when it should.
//!
//! The archive here is built exactly the way
//! `right_openshell::openshell::ssh_tar_download` builds it — members rooted at
//! `sandbox/`, foreign numeric uids — so the extraction flags are under test,
//! not a hand-tailored fixture.

use anyhow::{Context, Result};
use right_agent::sandbox_migrate::{
    MIGRATION_EXCLUDES, carried_entries, hand_home_to_guest_user, restore_archive, verify_restore,
};
use right_sandbox::{
    DEFAULT_READY_TIMEOUT, DEFAULT_SANDBOX_IMAGE, ExecRequest, GUEST_HOME, SandboxHandle,
    SandboxSpec, ensure_runtime_installed, fit_sandbox_name,
};

/// Top-level entries the fake OpenShell home holds. `.cache` is excluded from
/// the archive; `.platform` must survive as root-owned.
const SOURCE_LISTING: &str = ".platform\n.claude\n.cache\nCLAUDE.md\nprojects\n";

/// Uid the archive claims for every member: a foreign id from the OpenShell
/// guest that must not survive into the new sandbox.
const FOREIGN_UID: &str = "1234";

/// Deletes the sandbox even when the test panics mid-assertion.
struct SandboxGuard(String);

impl Drop for SandboxGuard {
    fn drop(&mut self) {
        let name = self.0.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("cleanup runtime");
            if let Err(error) = runtime.block_on(SandboxHandle::delete(&name)) {
                eprintln!("cleanup: deleting {name} failed: {error:#}");
            }
        })
        .join()
        .expect("cleanup thread");
    }
}

fn sandbox_name(label: &str) -> String {
    let runid =
        std::env::var("RIGHT_TEST_RUN_ID").unwrap_or_else(|_| std::process::id().to_string());
    fit_sandbox_name(&format!("rt-msbmig-{label}-{runid}"))
}

async fn boot(name: &str) -> Result<SandboxHandle> {
    let mut spec = SandboxSpec::new(name, DEFAULT_SANDBOX_IMAGE);
    spec.workdir = Some(GUEST_HOME.to_owned());
    let sandbox = SandboxHandle::create_or_attach(&spec)
        .await
        .context("create sandbox")?;
    sandbox
        .wait_ready(DEFAULT_READY_TIMEOUT)
        .await
        .context("wait ready")?;
    Ok(sandbox)
}

async fn sh(sandbox: &SandboxHandle, script: &str) -> Result<(i32, String)> {
    let request = ExecRequest {
        cmd: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        user: Some("0".to_owned()),
        timeout: Some(std::time::Duration::from_secs(120)),
        ..ExecRequest::default()
    };
    let outcome = sandbox.exec(&request).await.context("exec")?;
    Ok((
        outcome.code,
        String::from_utf8_lossy(&outcome.stdout).into_owned(),
    ))
}

/// Build the archive an OpenShell sandbox would have produced: `sandbox/…`
/// members owned by a uid that means nothing in the target image.
fn build_source_archive(dir: &std::path::Path) -> Result<std::path::PathBuf> {
    let home = dir.join("sandbox");
    std::fs::create_dir_all(home.join(".platform"))?;
    std::fs::create_dir_all(home.join(".claude"))?;
    std::fs::create_dir_all(home.join(".cache"))?;
    std::fs::create_dir_all(home.join("projects"))?;
    std::fs::write(home.join("CLAUDE.md"), "# migrated identity\n")?;
    std::fs::write(home.join(".claude/settings.json"), "{}\n")?;
    std::fs::write(home.join(".platform/marker"), "platform-owned\n")?;
    std::fs::write(home.join(".cache/huge.bin"), vec![0u8; 1024])?;
    std::fs::write(home.join("projects/notes.md"), "notes\n")?;

    let archive = dir.join("sandbox.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("czpf")
        .arg(&archive)
        .arg("-C")
        .arg(dir)
        .arg(format!("--owner={FOREIGN_UID}"))
        .arg(format!("--group={FOREIGN_UID}"))
        .args(
            MIGRATION_EXCLUDES
                .iter()
                .map(|path| format!("--exclude=sandbox/{path}")),
        )
        .arg("sandbox")
        .status()
        .context("run tar")?;
    anyhow::ensure!(status.success(), "tar failed: {status}");
    Ok(archive)
}

#[tokio::test]
#[ignore = "ci-msb: restores a migration archive into a live microVM"]
async fn ci_msb_migration_archive_restores_and_verifies() -> Result<()> {
    ensure_runtime_installed().await?;
    let dir = tempfile::tempdir()?;
    let archive = build_source_archive(dir.path())?;
    let expected = carried_entries(SOURCE_LISTING, MIGRATION_EXCLUDES);

    let name = sandbox_name("ok");
    let _guard = SandboxGuard(name.clone());
    let sandbox = boot(&name).await?;

    // The guest user exists in production because provisioning creates it;
    // create it here so the ownership hand-over is exercised, not skipped.
    let (code, _) = sh(
        &sandbox,
        "id -u sandbox >/dev/null 2>&1 || useradd -M -d /sandbox sandbox",
    )
    .await?;
    anyhow::ensure!(code == 0, "creating the guest user failed");

    restore_archive(&sandbox, &archive)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    let handed = hand_home_to_guest_user(&sandbox)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    anyhow::ensure!(handed, "the guest user exists, so the hand-over must run");

    verify_restore(&sandbox, &expected, handed)
        .await
        .map_err(|e| anyhow::anyhow!("verification of a good restore failed: {e:#}"))?;

    // Content came across.
    let (code, content) = sh(&sandbox, "cat /sandbox/CLAUDE.md").await?;
    anyhow::ensure!(code == 0, "reading a migrated file failed");
    assert_eq!(content.trim(), "# migrated identity");

    // Excluded caches did not.
    let (code, _) = sh(&sandbox, "test -e /sandbox/.cache").await?;
    assert_ne!(
        code, 0,
        "an excluded directory was carried into the sandbox"
    );

    // The foreign uid is gone: agent data belongs to the guest user, and
    // `.platform` stays root-owned.
    let (code, owners) = sh(
        &sandbox,
        "stat -c '%U' /sandbox/CLAUDE.md /sandbox/.claude /sandbox/projects /sandbox/.platform",
    )
    .await?;
    anyhow::ensure!(code == 0, "stat failed");
    assert_eq!(
        owners.split_whitespace().collect::<Vec<_>>(),
        vec!["sandbox", "sandbox", "sandbox", "root"],
        "uid remap did not land"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "ci-msb: proves migration verification fails on a partial restore"]
async fn ci_msb_migration_verification_rejects_a_partial_restore() -> Result<()> {
    ensure_runtime_installed().await?;
    let dir = tempfile::tempdir()?;
    let archive = build_source_archive(dir.path())?;
    let expected = carried_entries(SOURCE_LISTING, MIGRATION_EXCLUDES);

    let name = sandbox_name("bad");
    let _guard = SandboxGuard(name.clone());
    let sandbox = boot(&name).await?;

    restore_archive(&sandbox, &archive)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;

    // Lose one carried entry the way a truncated stream would.
    let (code, _) = sh(&sandbox, "rm -rf /sandbox/projects").await?;
    anyhow::ensure!(code == 0, "removing an entry failed");

    let error = verify_restore(&sandbox, &expected, false)
        .await
        .expect_err("verification must reject a home that lost an entry");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("projects"),
        "the error must name the missing entry: {rendered}"
    );
    Ok(())
}
