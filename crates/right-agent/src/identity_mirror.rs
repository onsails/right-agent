use std::path::Path;

/// Agent-authored identity files that must have an explicit host mirror.
///
/// For sandboxed agents the authoritative runtime copy is under `/sandbox`.
/// The host copy is a control-plane/debug/rebootstrap mirror, not the prompt
/// source for sandboxed runtime.
pub const IDENTITY_MIRROR_FILES: [&str; 3] = ["IDENTITY.md", "SOUL.md", "USER.md"];

/// Return true when every required identity mirror file exists on host.
pub fn host_identity_mirror_complete(agent_dir: &Path) -> bool {
    IDENTITY_MIRROR_FILES
        .iter()
        .all(|name| agent_dir.join(name).exists())
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
