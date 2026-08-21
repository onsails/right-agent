//! Live-microVM probe for the cron fast-exit shape: the exact command form the
//! cron wrapper spawns (`/bin/sh -c '<script>'` as the guest user, null stdin,
//! piped stdout/stderr) where the script aborts in milliseconds without ever
//! producing a terminal `result` line. The bot's `consume_cron_stream` must see
//! EOF and the pump must see `Exited` — this is the transport half of that
//! contract, exercised without the bot.
//!
//! Boots a real microVM, so it is `#[ignore]`d behind the `ci-msb` marker and
//! run with `cargo nextest run -p right-sandbox --run-ignored all`.

mod common;

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use common::{
    NODE_IMAGE, PROBE_CPUS, PROBE_MEMORY_MIB, SandboxGuard, acquire_vm_slot,
    ensure_runtime_installed,
};
use right_sandbox::{
    ExecEvent, ExecRequest, Resources, SandboxHandle, SandboxSpec, Stdin,
};

/// How long the fast-exit observation may take before the probe calls it a
/// transport hang (the production symptom: exit never observed, task parked).
const FAST_EXIT_OBSERVE_TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::test]
#[ignore = "ci-msb: boots a live microVM and observes a fast non-zero guest exit"]
async fn ci_msb_fast_exit_stream_delivers_exited() -> Result<()> {
    ensure_runtime_installed().await?;
    let _slot = acquire_vm_slot();
    let guard = SandboxGuard::new("fastexit");

    // No create-time `user`: the stock image has no `sandbox` user, and
    // pinning one kills guest init before the agent relay is up (the pilot
    // boot failure `agent_sandbox_spec` deliberately avoids). The probe runs
    // plain `/bin/sh`, which does not exist as any particular user's shell —
    // dash behaves identically for the constructs under test.
    let mut spec = SandboxSpec::new(guard.name(), NODE_IMAGE);
    spec.resources = Resources {
        cpus: PROBE_CPUS,
        memory_mib: PROBE_MEMORY_MIB,
        writable_layer_mib: spec.resources.writable_layer_mib,
    };
    let handle = SandboxHandle::create_or_attach(&spec)
        .await
        .context("boot node sandbox")?;

    // The exact cron-wrapper shape under the pre-fix bug: dash rejects
    // `set -o pipefail` and exits 2 with the reason on stderr, no stdout.
    let request = ExecRequest {
        cmd: "/bin/sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "set -o pipefail\nmkdir -p /tmp/crons-logs\necho body | tee /tmp/crons-logs/x.ndjson".to_owned(),
        ],
        user: None,
        stdin: Stdin::Null,
        timeout: None,
        env: Vec::new(),
        cwd: None,
    };

    let started = Instant::now();
    let mut stream = handle
        .exec_stream(&request)
        .await
        .context("start fast-exit exec session")?;

    let mut saw_stderr = false;
    let exit_code = loop {
        let event = tokio::time::timeout(FAST_EXIT_OBSERVE_TIMEOUT, stream.next_event())
            .await
            .context("no exec event within the observe window — exit never delivered")?
            .context("next_event failed")?;
        match event {
            Some(ExecEvent::Stderr(bytes)) => {
                assert!(
                    bytes.windows(b"pipefail".len()).any(|w| w == b"pipefail"),
                    "expected the dash pipefail diagnostic on stderr, got: {bytes:?}"
                );
                saw_stderr = true;
            }
            Some(ExecEvent::Exited { code }) => break Some(code),
            Some(_) => {}
            None => break None,
        }
    };

    assert!(
        saw_stderr,
        "the dash error diagnostic must arrive on stderr before exit"
    );
    let code = exit_code.context("stream ended without an Exited event")?;
    assert_eq!(code, 2, "dash must exit 2 on the illegal option");
    assert!(
        started.elapsed() < FAST_EXIT_OBSERVE_TIMEOUT,
        "exit must be observed promptly, took {:?}",
        started.elapsed()
    );

    // And the healthy shape after the fix: the same wrapper minus pipefail
    // runs to completion under dash.
    let ok_request = ExecRequest {
        cmd: "/bin/sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "mkdir -p /tmp/crons-logs\nprintf 'body\\n' | tee /tmp/crons-logs/ok.ndjson".to_owned(),
        ],
        user: None,
        stdin: Stdin::Null,
        timeout: None,
        env: Vec::new(),
        cwd: None,
    };
    let mut ok_stream = handle.exec_stream(&ok_request).await?;
    let mut ok_stdout = String::new();
    let ok_code = loop {
        let event = tokio::time::timeout(FAST_EXIT_OBSERVE_TIMEOUT, ok_stream.next_event())
            .await
            .context("no event for the healthy wrapper within the observe window")?
            .context("next_event failed")?;
        match event {
            Some(ExecEvent::Stdout(bytes)) => {
                ok_stdout.push_str(&String::from_utf8_lossy(&bytes))
            }
            Some(ExecEvent::Exited { code }) => break code,
            Some(_) => {}
            None => bail!("healthy wrapper stream ended without Exited"),
        }
    };
    assert_eq!(ok_code, 0, "post-fix wrapper must exit 0 under dash");
    assert!(
        ok_stdout.contains("body"),
        "tee output must stream back, got: {ok_stdout:?}"
    );

    handle.destroy().await.context("destroy probe sandbox")?;
    Ok(())
}
