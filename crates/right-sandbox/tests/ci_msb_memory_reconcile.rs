//! Live-microVM probe for [`SandboxHandle::reconcile_memory`]: the one
//! behavior that cannot be unit-tested without a real VM. The SDK's live
//! resize cannot move memory *below* boot memory (the runtime clamps to boot),
//! so a shrink must be persisted and then stop/started. This proves the
//! reconcile does exactly that: a no-op leaves the boot id alone, and a shrink
//! re-boots the guest at the smaller size.
//!
//! Boots a real microVM, so it is `#[ignore]`d behind the `ci-msb` marker and
//! run with `cargo nextest run -p right-sandbox --run-ignored all`.

mod common;

use anyhow::{Context, Result};
use common::{NODE_IMAGE, SandboxGuard, acquire_vm_slot, ensure_runtime_installed};
use right_sandbox::{ExecRequest, MemoryReconcile, Resources, SandboxHandle, SandboxSpec};

/// Read the guest's boot id, which changes if and only if the VM rebooted.
async fn guest_boot_id(sandbox: &SandboxHandle) -> Result<String> {
    let mut request = ExecRequest::new("cat");
    request.args = vec!["/proc/sys/kernel/random/boot_id".to_owned()];
    request.user = Some("0".to_owned());
    let output = sandbox.exec(&request).await.context("read guest boot id")?;
    assert_eq!(output.code, 0, "read guest boot id: {output:?}");
    Ok(String::from_utf8(output.stdout)
        .context("guest boot id is not utf-8")?
        .trim()
        .to_owned())
}

/// Read the guest's `MemTotal` in KiB.
async fn guest_memtotal_kib(sandbox: &SandboxHandle) -> Result<u64> {
    let mut request = ExecRequest::new("cat");
    request.args = vec!["/proc/meminfo".to_owned()];
    request.user = Some("0".to_owned());
    let output = sandbox.exec(&request).await.context("read /proc/meminfo")?;
    assert_eq!(output.code, 0, "read /proc/meminfo: {output:?}");
    let text = String::from_utf8(output.stdout).context("meminfo is not utf-8")?;
    let line = text
        .lines()
        .find(|l| l.starts_with("MemTotal:"))
        .context("MemTotal missing from /proc/meminfo")?;
    line.split_whitespace()
        .nth(1)
        .context("MemTotal has no value")?
        .parse()
        .context("MemTotal is not a number")
}

#[tokio::test]
#[ignore = "ci-msb: boots a live microVM and verifies a memory reconcile re-boots it at the new size"]
async fn ci_msb_memory_reconcile_shrink_reboots_at_target() -> Result<()> {
    ensure_runtime_installed().await?;
    let _slot = acquire_vm_slot();
    let guard = SandboxGuard::new("memreconcile");

    let mut spec = SandboxSpec::new(guard.name(), NODE_IMAGE);
    spec.resources = Resources {
        memory_mib: 1024,
        ..spec.resources
    };
    let handle = SandboxHandle::create_or_attach(&spec)
        .await
        .context("boot sandbox")?;

    // Already at the target: no-op, and no reboot.
    let boot_before = guest_boot_id(&handle).await?;
    let unchanged = handle
        .reconcile_memory(1024)
        .await
        .context("no-op reconcile")?;
    assert_eq!(
        unchanged,
        MemoryReconcile::Unchanged,
        "equal target is a no-op"
    );
    assert_eq!(
        boot_before,
        guest_boot_id(&handle).await?,
        "a no-op reconcile must not reboot the guest"
    );

    // Shrink below boot memory: must restart and re-boot at the smaller size.
    let resized = handle
        .reconcile_memory(512)
        .await
        .context("shrink reconcile")?;
    assert_eq!(
        resized,
        MemoryReconcile::ResizedWithRestart,
        "a shrink is restart-backed"
    );
    assert_ne!(
        boot_before,
        guest_boot_id(&handle).await?,
        "a shrink must reboot the guest"
    );
    let memtotal = guest_memtotal_kib(&handle).await?;
    assert!(
        memtotal < 700_000,
        "guest must re-boot below 1 GiB, got {memtotal} kB"
    );

    Ok(())
}
