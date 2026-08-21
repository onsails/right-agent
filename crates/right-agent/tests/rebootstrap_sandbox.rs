//! Integration test: `rebootstrap::execute` against a live Agent Sandbox.
//!
//! The command's whole guarantee is an ordering one — the authoritative
//! sandbox identity is deleted *before* any host state is touched — and that
//! ordering is only observable end to end. Boots one small microVM, so it is
//! `#[ignore]`d behind the `ci-msb` marker like every other live-microVM
//! probe: `cargo nextest run -p right-agent --run-ignored all`.

use std::path::Path;
use std::sync::Arc;

use right_agent::rebootstrap::{self, IDENTITY_FILES, RebootstrapPlan};
use right_sandbox::{SandboxHandle, SandboxSpec};

/// Small arm64-native guest image; this probe needs a filesystem, not a
/// toolchain.
const PROBE_IMAGE: &str = "alpine:3";

/// Guest directory the agent's authoritative files live in.
const GUEST_HOME: &str = "/sandbox";

/// Owns a probe sandbox and deletes it on drop.
///
/// Cleanup runs on a dedicated thread with its own current-thread runtime so
/// it works from inside an async test and from a panicking unwind alike.
struct SandboxGuard(Option<Arc<SandboxHandle>>);

impl SandboxGuard {
    fn handle(&self) -> &SandboxHandle {
        self.0.as_ref().expect("guard is armed")
    }
}

impl Drop for SandboxGuard {
    fn drop(&mut self) {
        let Some(handle) = self.0.take() else {
            return;
        };
        let name = handle.name().to_owned();
        let outcome = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build cleanup runtime")
                .block_on(handle.destroy())
        })
        .join();

        let error = match outcome {
            Ok(Ok(())) => return,
            Ok(Err(e)) => format!("{e:#}"),
            Err(_) => "cleanup thread panicked".to_owned(),
        };
        // Never mask the original failure by panicking during an unwind, but
        // never let a leaked microVM pass silently either.
        eprintln!("SandboxGuard: failed to remove sandbox {name}: {error}");
        assert!(
            std::thread::panicking(),
            "SandboxGuard: failed to remove sandbox {name}: {error}"
        );
    }
}

/// A sandbox name unique across processes and runs.
fn unique_sandbox_name() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_millis();
    format!("rt-msb-rebootstrap-{}-{millis}", std::process::id())
}

/// Write a host-side agent dir with `agent.yaml` pointing at `sandbox_name`,
/// the three identity files, and a stamped active session row in data.db.
async fn seed_agent_dir(agent_dir: &Path, sandbox_name: &str) {
    std::fs::create_dir_all(agent_dir).unwrap();
    let yaml = format!("sandbox:\n  name: {sandbox_name}\n");
    std::fs::write(agent_dir.join("agent.yaml"), yaml).unwrap();
    std::fs::write(agent_dir.join("IDENTITY.md"), "host id\n").unwrap();
    std::fs::write(agent_dir.join("SOUL.md"), "host soul\n").unwrap();
    std::fs::write(agent_dir.join("USER.md"), "host user\n").unwrap();

    let conn = right_db::open_connection(agent_dir, true).await.unwrap();
    conn.execute(
        "INSERT INTO sessions (chat_id, thread_id, root_session_id, is_active) \
         VALUES (1, 0, 'sandbox-session-uuid', 1)",
        [],
    )
    .await
    .unwrap();
}

#[ignore = "ci-msb: boots a real microVM"]
#[tokio::test]
async fn ci_msb_execute_against_live_sandbox() {
    right_sandbox::ensure_runtime_installed()
        .await
        .expect("install pinned microsandbox runtime");

    let sandbox_name = unique_sandbox_name();
    let mut spec = SandboxSpec::new(&sandbox_name, PROBE_IMAGE);
    spec.workdir = Some(GUEST_HOME.to_owned());
    let guard = SandboxGuard(Some(Arc::new(
        SandboxHandle::create_or_attach(&spec)
            .await
            .expect("create probe sandbox"),
    )));
    let sandbox = guard.handle();

    // Seed sandbox-side identity files. `/sandbox` is the workdir but the
    // stock image does not ship it.
    sandbox.fs_mkdir(GUEST_HOME).await.expect("mkdir /sandbox");
    for &f in IDENTITY_FILES {
        sandbox
            .fs_write(
                &format!("{GUEST_HOME}/{f}"),
                format!("sandbox-{f}\n").as_bytes(),
            )
            .await
            .unwrap_or_else(|e| panic!("seed /sandbox/{f}: {e:#}"));
    }

    // Set up a temp home with the agent dir under it.
    let home = tempfile::tempdir().unwrap();
    let agent_name = "rb-test";
    let agent_dir = home.path().join("agents").join(agent_name);
    seed_agent_dir(&agent_dir, &sandbox_name).await;

    // Build the plan manually: `plan()` resolves the sandbox name from
    // `agent.yaml`, but this probe's sandbox carries a randomised name.
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M").to_string();
    let p = RebootstrapPlan {
        agent_name: agent_name.to_string(),
        agent_dir: agent_dir.clone(),
        backup_dir: home
            .path()
            .join("backups")
            .join(agent_name)
            .join(format!("rebootstrap-{timestamp}")),
        sandbox_name: sandbox_name.clone(),
    };

    let report = rebootstrap::execute(&p).await.expect("execute failed");

    // Host: identity files removed
    for &f in IDENTITY_FILES {
        assert!(!agent_dir.join(f).exists(), "host {f} should be removed");
    }
    // Host: BOOTSTRAP.md created
    let bootstrap = std::fs::read_to_string(agent_dir.join("BOOTSTRAP.md")).unwrap();
    assert_eq!(bootstrap, right_codegen::BOOTSTRAP_INSTRUCTIONS);

    // Backup: host copies (use concrete content map — what seed_agent_dir wrote)
    let expected_host: &[(&str, &str)] = &[
        ("IDENTITY.md", "host id\n"),
        ("SOUL.md", "host soul\n"),
        ("USER.md", "host user\n"),
    ];
    for (name, content) in expected_host {
        let host_copy = report.backup_dir.join(name);
        assert!(host_copy.exists(), "backup of host {name} missing");
        assert_eq!(&std::fs::read_to_string(&host_copy).unwrap(), content);
    }

    // Backup: sandbox copies
    for &f in IDENTITY_FILES {
        let sb_copy = report.backup_dir.join("sandbox").join(f);
        assert!(sb_copy.exists(), "backup of sandbox {f} missing");
        let content = std::fs::read_to_string(&sb_copy).unwrap();
        assert_eq!(content, format!("sandbox-{f}\n"));
    }

    // Sandbox: identity files removed. This is the authoritative copy — the
    // host reset above is only correct because this one is gone.
    for &f in IDENTITY_FILES {
        let guest_path = format!("{GUEST_HOME}/{f}");
        assert!(
            !sandbox.fs_exists(&guest_path).await.unwrap(),
            "expected {guest_path} to be absent in sandbox"
        );
    }

    assert_eq!(report.sessions_deactivated, 1);
    assert_eq!(report.host_backed_up.to_vec(), IDENTITY_FILES.to_vec());
    assert_eq!(report.sandbox_backed_up.to_vec(), IDENTITY_FILES.to_vec());
}
