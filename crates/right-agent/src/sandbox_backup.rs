//! Archiving an Agent Sandbox's guest home into a `sandbox.tar.gz`.
//!
//! One implementation shared by `right agent backup` and the pre-destroy
//! backup in [`crate::agent::destroy`]: both write the archive layout that
//! `right agent restore` extracts with `--strip-components=1 sandbox`, and a
//! second copy of that layout is a restore that silently produces an empty
//! agent.

use std::path::Path;
use std::time::Duration;

use right_sandbox::{ExecRequest, GUEST_HOME, SandboxHandle};

/// Guest-home entries left out of the archive: package and build caches that
/// a rebuild regenerates. Excluding them keeps a backup to the data the user
/// cannot recreate.
const REBUILDABLE_BACKUP_EXCLUDES: &[&str] = &[".cache", ".venv", ".npm", ".uv"];

/// Where the guest writes the archive before it is copied to the host.
const GUEST_ARCHIVE_PATH: &str = "/tmp/right-sandbox-backup.tar.gz";

/// Cap on the in-guest `tar` run. Archiving a working agent home is seconds;
/// this only bounds a pathological one.
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(300);

/// Archive the sandbox's guest home to `dest_tar` on the host.
///
/// Archives inside the guest and copies one file out: the guest home is
/// thousands of files and `fs_copy_to_host` moves them one at a time. Members
/// are rooted at `sandbox/`, the layout `right agent restore` expects.
///
/// `include_rebuildable` keeps the package and build caches in
/// [`REBUILDABLE_BACKUP_EXCLUDES`]; the default excludes them, so an
/// unattended backup carries only data a rebuild cannot recreate.
///
/// Every failure is an error. The guest holds the agent's authoritative
/// memory, skills and workspace, so a caller that is about to delete the
/// sandbox must not mistake a partial archive for a backup.
pub async fn archive_guest_home(
    sandbox: &SandboxHandle,
    dest_tar: &Path,
    include_rebuildable: bool,
) -> miette::Result<()> {
    let name = sandbox.name();
    let archive_root = GUEST_HOME.trim_start_matches('/');

    let mut args = vec![
        "czf".to_owned(),
        GUEST_ARCHIVE_PATH.to_owned(),
        "-C".to_owned(),
        "/".to_owned(),
    ];
    if !include_rebuildable {
        for excluded in REBUILDABLE_BACKUP_EXCLUDES {
            args.push(format!("--exclude={archive_root}/{excluded}"));
            args.push(format!("--exclude={archive_root}/{excluded}/*"));
        }
    }
    args.push(archive_root.to_owned());

    let request = ExecRequest {
        cmd: "tar".to_owned(),
        args,
        // Root reads every file in the guest home regardless of owner; a
        // partial archive would be worse than none.
        user: Some("0".to_owned()),
        timeout: Some(ARCHIVE_TIMEOUT),
        ..ExecRequest::default()
    };
    let outcome = sandbox
        .exec(&request)
        .await
        .map_err(|error| miette::miette!("archive sandbox '{name}': {error:#}"))?;
    if !outcome.success() {
        return Err(miette::miette!(
            "archiving sandbox '{name}' exited with {}: {}",
            outcome.code,
            String::from_utf8_lossy(&outcome.stderr).trim(),
        ));
    }

    sandbox
        .fs_copy_to_host(GUEST_ARCHIVE_PATH, dest_tar)
        .await
        .map_err(|error| {
            miette::miette!(
                "download the archive of sandbox '{name}' to {}: {error:#}",
                dest_tar.display()
            )
        })?;
    sandbox
        .fs_remove(GUEST_ARCHIVE_PATH)
        .await
        .map_err(|error| {
            miette::miette!("remove {GUEST_ARCHIVE_PATH} from sandbox '{name}': {error:#}")
        })
}
