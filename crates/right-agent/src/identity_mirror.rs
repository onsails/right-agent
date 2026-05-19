use std::path::Path;

/// Agent-authored identity files that must have an explicit host mirror.
///
/// For sandboxed agents the authoritative runtime copy is under `/sandbox`.
/// The host copy is a control-plane/debug/rebootstrap mirror, not the prompt
/// source for sandboxed runtime.
pub const IDENTITY_MIRROR_FILES: [&str; 3] = ["IDENTITY.md", "SOUL.md", "USER.md"];

const SOUL_OPERATING_CONTRACT_MARKER: &str = "RIGHT_AGENT:SOUL_OPERATING_CONTRACT v1 START";

const SOUL_OPERATING_CONTRACT_BLOCK: &str = "\
<!-- RIGHT_AGENT:SOUL_OPERATING_CONTRACT v1 START -->
## Operating Contract

- Act on reversible, low-risk work without ceremony.
- Ask before public, costly, destructive, credential/security, or private-data actions.
- Challenge weak assumptions with evidence.
- Prefer usable outcomes over polished artifacts.
<!-- RIGHT_AGENT:SOUL_OPERATING_CONTRACT v1 END -->
";

/// Return true when every required identity mirror file exists on host.
pub fn host_identity_mirror_complete(agent_dir: &Path) -> bool {
    IDENTITY_MIRROR_FILES
        .iter()
        .all(|name| agent_dir.join(name).exists())
}

/// Append the managed SOUL.md operating contract when an existing SOUL.md lacks it.
pub fn with_soul_operating_contract(content: &str) -> Option<String> {
    if content.contains(SOUL_OPERATING_CONTRACT_MARKER) {
        return None;
    }

    let mut migrated = content.to_owned();
    if !migrated.ends_with('\n') {
        migrated.push('\n');
    }
    migrated.push('\n');
    migrated.push_str(SOUL_OPERATING_CONTRACT_BLOCK);
    Some(migrated)
}

/// Add the managed operating-contract block to a host SOUL.md mirror if present.
pub fn migrate_host_soul_operating_contract(agent_dir: &Path) -> std::io::Result<bool> {
    let soul_path = agent_dir.join("SOUL.md");
    if !soul_path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(&soul_path)?;
    let Some(migrated) = with_soul_operating_contract(&content) else {
        return Ok(false);
    };

    std::fs::write(soul_path, migrated)?;
    Ok(true)
}

/// Download authoritative sandbox identity files into the host agent directory.
///
/// This is an explicit reconciliation step. It intentionally does not include
/// `TOOLS.md`: sandbox prompt runtime reads `/sandbox/TOOLS.md`, and current
/// host-side consumers only require identity files.
pub async fn sync_identity_mirror_from_sandbox(
    agent_dir: &Path,
    sandbox_name: &str,
) -> miette::Result<()> {
    let mut set = tokio::task::JoinSet::new();
    for filename in IDENTITY_MIRROR_FILES {
        let sandbox = sandbox_name.to_owned();
        let host_dest = agent_dir.join(filename);
        set.spawn(async move {
            let sandbox_path = format!("/sandbox/{filename}");
            right_openshell::openshell::download_file(&sandbox, &sandbox_path, &host_dest)
                .await
                .map_err(|e| format!("{filename}: {e:#}"))
        });
    }

    let mut errors = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => errors.push(msg),
            Err(join_err) => errors.push(format!("task panicked: {join_err}")),
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(miette::miette!(
            "identity mirror sync from sandbox '{}' failed: {}",
            sandbox_name,
            errors.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_openshell::test_support::{PROCESS_ENV_LOCK, PathGuard};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn host_identity_mirror_requires_only_identity_files() {
        assert_eq!(IDENTITY_MIRROR_FILES, ["IDENTITY.md", "SOUL.md", "USER.md"]);
        assert!(!IDENTITY_MIRROR_FILES.contains(&"TOOLS.md"));
    }

    #[test]
    fn host_identity_mirror_complete_requires_all_identity_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!host_identity_mirror_complete(dir.path()));

        std::fs::write(dir.path().join("IDENTITY.md"), "# identity\n").unwrap();
        std::fs::write(dir.path().join("SOUL.md"), "# soul\n").unwrap();
        assert!(!host_identity_mirror_complete(dir.path()));

        std::fs::write(dir.path().join("USER.md"), "# user\n").unwrap();
        assert!(host_identity_mirror_complete(dir.path()));
    }

    #[test]
    fn soul_operating_contract_migration_appends_managed_block() {
        let existing = "# SOUL\n\n- Direct and pragmatic.\n";
        let migrated = with_soul_operating_contract(existing)
            .expect("missing operating contract should be migrated");

        assert!(migrated.starts_with(existing));
        assert!(migrated.contains("RIGHT_AGENT:SOUL_OPERATING_CONTRACT v1 START"));
        assert!(migrated.contains("## Operating Contract"));
        assert!(migrated.contains("reversible, low-risk work"));
        assert!(migrated.contains("credential/security"));
        assert!(migrated.contains("usable outcomes over polished artifacts"));
    }

    #[test]
    fn soul_operating_contract_migration_is_idempotent() {
        let once = with_soul_operating_contract("# SOUL\n").expect("first migration");

        assert!(
            with_soul_operating_contract(&once).is_none(),
            "second migration must not duplicate the managed block"
        );
    }

    #[test]
    fn host_soul_operating_contract_migration_skips_missing_soul() {
        let dir = tempfile::tempdir().unwrap();

        let migrated = migrate_host_soul_operating_contract(dir.path()).unwrap();

        assert!(!migrated);
        assert!(!dir.path().join("SOUL.md").exists());
    }

    #[test]
    fn host_soul_operating_contract_migration_updates_existing_soul_once() {
        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("SOUL.md");
        std::fs::write(&soul_path, "# SOUL\n").unwrap();

        assert!(migrate_host_soul_operating_contract(dir.path()).unwrap());
        assert!(!migrate_host_soul_operating_contract(dir.path()).unwrap());

        let migrated = std::fs::read_to_string(&soul_path).unwrap();
        assert_eq!(
            migrated
                .matches("RIGHT_AGENT:SOUL_OPERATING_CONTRACT v1 START")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn sync_identity_mirror_from_sandbox_downloads_required_files() {
        let _guard = PROCESS_ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let fake_openshell = bin.join("openshell");
        std::fs::write(
            &fake_openshell,
            r#"#!/bin/sh
set -eu
if [ "$1" != "sandbox" ] || [ "$2" != "download" ]; then
  exit 64
fi
sandbox="$3"
src="$4"
dest="$5"
if [ "$sandbox" != "right-test-sandbox" ]; then
  exit 65
fi
case "$src" in
  /sandbox/IDENTITY.md) printf '# identity\n' > "$dest/IDENTITY.md" ;;
  /sandbox/SOUL.md) printf '# soul\n' > "$dest/SOUL.md" ;;
  /sandbox/USER.md) printf '# user\n' > "$dest/USER.md" ;;
  *) exit 66 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake_openshell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _path_guard = PathGuard::prepend(&bin);

        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir(&agent_dir).unwrap();

        sync_identity_mirror_from_sandbox(&agent_dir, "right-test-sandbox")
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(agent_dir.join("IDENTITY.md")).unwrap(),
            "# identity\n"
        );
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("SOUL.md")).unwrap(),
            "# soul\n"
        );
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("USER.md")).unwrap(),
            "# user\n"
        );
    }

    #[tokio::test]
    async fn sync_identity_mirror_from_sandbox_fails_when_any_required_file_is_missing() {
        let _guard = PROCESS_ENV_LOCK.lock().await;

        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let fake_openshell = bin.join("openshell");
        std::fs::write(
            &fake_openshell,
            r#"#!/bin/sh
set -eu
src="$4"
dest="$5"
case "$src" in
  /sandbox/IDENTITY.md) printf '# identity\n' > "$dest/IDENTITY.md" ;;
  /sandbox/SOUL.md) exit 1 ;;
  /sandbox/USER.md) printf '# user\n' > "$dest/USER.md" ;;
  *) exit 66 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake_openshell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _path_guard = PathGuard::prepend(&bin);

        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir(&agent_dir).unwrap();

        let err = sync_identity_mirror_from_sandbox(&agent_dir, "right-test-sandbox")
            .await
            .expect_err("missing SOUL.md must fail reconciliation");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("SOUL.md"),
            "error should name missing file: {msg}"
        );
        assert!(!host_identity_mirror_complete(&agent_dir));
    }
}
