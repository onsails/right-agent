//! Guest-side half of `right agent migrate-sandbox`: unpack an agent's
//! OpenShell home into a microsandbox VM and prove it landed.
//!
//! The CLI owns the OpenShell side (archive, `agent.yaml` rewrite, deleting the
//! old sandbox); everything that touches the *new* sandbox lives here, beside
//! [`crate::sandbox_backup`], so it can be exercised against a real microVM.
//!
//! Nothing in this module is destructive: it only ever writes into the new
//! sandbox. The migration's one destructive step runs in the CLI, after
//! [`verify_restore`] returns `Ok`.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use right_sandbox::{ExecRequest, GUEST_HOME, GUEST_USER, SandboxHandle};

/// Guest-home-relative paths left out of the migration archive.
///
/// The rebuildable caches are the same set `right agent backup` skips; `.ssh`
/// held the retired OpenShell SSH transport's keys and `known_hosts`, which
/// mean nothing to a microVM that has no SSH at all. Everything else is agent
/// data and is carried verbatim.
pub const MIGRATION_EXCLUDES: &[&str] = &[".cache", ".venv", ".npm", ".uv", ".ssh"];

/// The one carried entry that stays root-owned after the restore.
///
/// `.platform` is the platform-owned tree the agent must not be able to
/// rewrite (provisioning `chmod a-w`s it); handing it to the guest user would
/// undo exactly that.
const PLATFORM_ENTRY: &str = ".platform";

/// Guest path the migration archive is staged at before extraction. Removed as
/// soon as `tar` has read it, so the migrated sandbox carries no copy of its
/// own archive.
const MIGRATE_TAR_GUEST_PATH: &str = "/tmp/right-migrate.tar.gz";

/// How long the in-guest extraction may take before it is killed.
const EXTRACT_TIMEOUT: Duration = Duration::from_secs(900);

/// How long the recursive ownership hand-over may take.
const OWNERSHIP_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a verification probe may take.
const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);

/// The top-level entries the archive actually carried.
///
/// `listing` is the source sandbox's `ls -A /sandbox` output; anything named by
/// [`MIGRATION_EXCLUDES`] was left out of the archive and must not be expected
/// on the other side. Nested excludes (`.claude/settings.json`) never match a
/// top-level name, so their parent stays required.
pub fn carried_entries(listing: &str, excludes: &[&str]) -> Vec<String> {
    listing
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter(|name| !excludes.contains(name))
        .map(str::to_owned)
        .collect()
}

/// Basename of a guest path, so a listing can be compared by entry name
/// whether the SDK reports `"/sandbox/.claude"` or `".claude"`.
pub fn entry_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

/// Entries the source had that the restored sandbox does not.
pub fn missing_entries<'a>(expected: &'a [String], present: &HashSet<&str>) -> Vec<&'a str> {
    expected
        .iter()
        .map(String::as_str)
        .filter(|name| !present.contains(name))
        .collect()
}

/// Run a guest command and fail with its stderr when it exits non-zero.
async fn run_checked(
    sandbox: &SandboxHandle,
    request: &ExecRequest,
    what: &str,
) -> miette::Result<String> {
    let outcome = sandbox
        .exec(request)
        .await
        .map_err(|error| miette::miette!("{what}: {error:#}"))?;
    if !outcome.success() {
        return Err(miette::miette!(
            "{what} exited with {}: {}",
            outcome.code,
            String::from_utf8_lossy(&outcome.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&outcome.stdout).into_owned())
}

/// Unpack the migration archive over the new sandbox's guest home.
///
/// `--no-same-owner` is the uid remap: the archive carries the OpenShell
/// guest's numeric uids, which mean something else (or nothing) in this image,
/// so extraction normalizes everything to root and
/// [`hand_home_to_guest_user`] then hands it to the guest user by name.
pub async fn restore_archive(sandbox: &SandboxHandle, archive: &Path) -> miette::Result<()> {
    sandbox
        .fs_copy_from_host(archive, MIGRATE_TAR_GUEST_PATH)
        .await
        .map_err(|error| {
            miette::miette!(
                "upload {} into the new sandbox: {error:#}",
                archive.display()
            )
        })?;

    // Provisioning is what normally creates the agent's home, and it has not
    // run yet on a freshly created sandbox — so the migration makes its own
    // extraction target. Root-owned here; `hand_home_to_guest_user` reassigns
    // everything except the platform tree once the archive is in place.
    let make_home = ExecRequest {
        cmd: "mkdir".to_owned(),
        args: vec!["-p".to_owned(), GUEST_HOME.to_owned()],
        user: Some("0".to_owned()),
        timeout: Some(EXTRACT_TIMEOUT),
        ..ExecRequest::default()
    };
    run_checked(sandbox, &make_home, "create the guest home").await?;

    let extract = ExecRequest {
        cmd: "tar".to_owned(),
        args: vec![
            "xzpf".to_owned(),
            MIGRATE_TAR_GUEST_PATH.to_owned(),
            "-C".to_owned(),
            GUEST_HOME.to_owned(),
            "--strip-components=1".to_owned(),
            "--no-same-owner".to_owned(),
            "sandbox".to_owned(),
        ],
        user: Some("0".to_owned()),
        timeout: Some(EXTRACT_TIMEOUT),
        ..ExecRequest::default()
    };
    run_checked(sandbox, &extract, "extract the migration archive").await?;

    sandbox
        .fs_remove(MIGRATE_TAR_GUEST_PATH)
        .await
        .map_err(|error| miette::miette!("remove the staged migration archive: {error:#}"))
}

/// Give every carried top-level entry except `.platform` to the guest user.
///
/// Returns whether the hand-over happened: the unprivileged guest user is
/// created by sandbox provisioning, and a sandbox that has not been provisioned
/// yet has no such user. Leaving the files root-owned is then the correct
/// outcome (provisioning owns that step), and the caller reports it rather than
/// pretending the ownership is what it is not.
pub async fn hand_home_to_guest_user(sandbox: &SandboxHandle) -> miette::Result<bool> {
    let probe = ExecRequest {
        cmd: "id".to_owned(),
        args: vec!["-u".to_owned(), GUEST_USER.to_owned()],
        user: Some("0".to_owned()),
        timeout: Some(VERIFY_TIMEOUT),
        ..ExecRequest::default()
    };
    let probe_outcome = sandbox
        .exec(&probe)
        .await
        .map_err(|error| miette::miette!("probe for the '{GUEST_USER}' guest user: {error:#}"))?;
    if !probe_outcome.success() {
        tracing::warn!(
            user = GUEST_USER,
            "guest user does not exist yet; migrated files stay root-owned until provisioning creates it"
        );
        return Ok(false);
    }

    let chown = ExecRequest {
        cmd: "find".to_owned(),
        args: vec![
            GUEST_HOME.to_owned(),
            "-mindepth".to_owned(),
            "1".to_owned(),
            "-maxdepth".to_owned(),
            "1".to_owned(),
            "!".to_owned(),
            "-name".to_owned(),
            PLATFORM_ENTRY.to_owned(),
            "-exec".to_owned(),
            "chown".to_owned(),
            "-R".to_owned(),
            GUEST_USER.to_owned(),
            "{}".to_owned(),
            "+".to_owned(),
        ],
        user: Some("0".to_owned()),
        timeout: Some(OWNERSHIP_TIMEOUT),
        ..ExecRequest::default()
    };
    run_checked(sandbox, &chown, "hand the migrated home to the guest user").await?;
    Ok(true)
}

/// Prove the restore landed before anything destructive runs.
///
/// Checks the three things a silent tar failure would break: every top-level
/// entry the source had is present, the home holds actual bytes, and — when the
/// hand-over ran — nothing outside `.platform` is still root-owned.
pub async fn verify_restore(
    sandbox: &SandboxHandle,
    expected: &[String],
    handed_to_guest: bool,
) -> miette::Result<()> {
    let entries = sandbox
        .fs_list(GUEST_HOME)
        .await
        .map_err(|error| miette::miette!("list the migrated guest home: {error:#}"))?;
    let present: HashSet<&str> = entries
        .iter()
        .map(|entry| entry_name(&entry.path))
        .collect();
    let missing = missing_entries(expected, &present);
    if !missing.is_empty() {
        return Err(miette::miette!(
            "the migrated sandbox is missing {} of {} entries the OpenShell sandbox had: {}",
            missing.len(),
            expected.len(),
            missing.join(", ")
        ));
    }

    let du = ExecRequest {
        cmd: "du".to_owned(),
        args: vec!["-s".to_owned(), "-k".to_owned(), GUEST_HOME.to_owned()],
        user: Some("0".to_owned()),
        timeout: Some(VERIFY_TIMEOUT),
        ..ExecRequest::default()
    };
    let du_out = run_checked(sandbox, &du, "measure the migrated guest home").await?;
    let kib: u64 = du_out
        .split_whitespace()
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or_else(|| miette::miette!("could not read `du` output: {}", du_out.trim()))?;
    if kib == 0 {
        return Err(miette::miette!(
            "the migrated guest home is empty after extraction"
        ));
    }

    if handed_to_guest {
        let stray = ExecRequest {
            cmd: "find".to_owned(),
            args: vec![
                GUEST_HOME.to_owned(),
                "-mindepth".to_owned(),
                "1".to_owned(),
                "-maxdepth".to_owned(),
                "1".to_owned(),
                "!".to_owned(),
                "-name".to_owned(),
                PLATFORM_ENTRY.to_owned(),
                "!".to_owned(),
                "-user".to_owned(),
                GUEST_USER.to_owned(),
                "-print".to_owned(),
            ],
            user: Some("0".to_owned()),
            timeout: Some(VERIFY_TIMEOUT),
            ..ExecRequest::default()
        };
        let stray_out = run_checked(sandbox, &stray, "check migrated ownership").await?;
        if !stray_out.trim().is_empty() {
            return Err(miette::miette!(
                "these migrated entries are not owned by '{GUEST_USER}': {}",
                stray_out.split_whitespace().collect::<Vec<_>>().join(", ")
            ));
        }
    }

    tracing::info!(
        entries = expected.len(),
        kib,
        handed_to_guest,
        "migration verified"
    );
    Ok(())
}
