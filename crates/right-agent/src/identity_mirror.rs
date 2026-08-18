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
    std::fs::create_dir_all(agent_dir).map_err(|e| {
        miette::miette!(
            "failed to create identity mirror directory {}: {e:#}",
            agent_dir.display()
        )
    })?;
    let staging = tempfile::tempdir_in(agent_dir).map_err(|e| {
        miette::miette!(
            "failed to create identity mirror staging directory in {}: {e:#}",
            agent_dir.display()
        )
    })?;

    let mut set = tokio::task::JoinSet::new();
    for filename in IDENTITY_MIRROR_FILES {
        let sandbox = sandbox_name.to_owned();
        let staged_dest = staging.path().join(filename);
        set.spawn(async move {
            let sandbox_path = format!("/sandbox/{filename}");
            right_openshell::openshell::download_file(&sandbox, &sandbox_path, &staged_dest)
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

    if !errors.is_empty() {
        return Err(miette::miette!(
            "identity mirror sync from sandbox '{}' failed: {}",
            sandbox_name,
            errors.join("; ")
        ));
    }

    publish_staged_identity_mirror(staging.path(), agent_dir, |source, destination| {
        std::fs::rename(source, destination)
    })
}

struct PublishedFile {
    destination: std::path::PathBuf,
    backup: Option<std::path::PathBuf>,
    published: bool,
}

fn publish_staged_identity_mirror<F>(
    staging: &Path,
    agent_dir: &Path,
    mut publish: F,
) -> miette::Result<()>
where
    F: FnMut(&Path, &Path) -> std::io::Result<()>,
{
    let mut planned_files = Vec::with_capacity(IDENTITY_MIRROR_FILES.len());

    for filename in IDENTITY_MIRROR_FILES {
        let staged = staging.join(filename);
        let destination = agent_dir.join(filename);
        let backup = match std::fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_file() => {
                Some(staging.join(format!(".backup-{filename}")))
            }
            Ok(_) => {
                return Err(miette::miette!(
                    "identity mirror destination {} is not a regular file",
                    destination.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(miette::miette!(
                    "failed to inspect identity mirror destination {}: {error:#}",
                    destination.display()
                ));
            }
        };
        planned_files.push((staged, destination, backup));
    }

    let mut files = Vec::with_capacity(planned_files.len());
    for (staged, destination, backup) in planned_files {
        if let Some(backup) = &backup
            && let Err(error) = std::fs::rename(&destination, backup)
        {
            return Err(publish_error(
                format!(
                    "failed to move identity mirror {} to backup {} before publish: {error:#}",
                    destination.display(),
                    backup.display()
                ),
                &files,
            ));
        }

        let file_index = files.len();
        files.push(PublishedFile {
            destination: destination.clone(),
            backup,
            published: false,
        });

        if let Err(error) = publish(&staged, &destination) {
            return Err(publish_error(
                format!(
                    "failed to publish identity mirror {} to {}: {error:#}",
                    staged.display(),
                    destination.display()
                ),
                &files,
            ));
        }
        files[file_index].published = true;
    }

    cleanup_identity_mirror_backups(&files)
}

fn cleanup_identity_mirror_backups(files: &[PublishedFile]) -> miette::Result<()> {
    let mut errors = Vec::new();
    for file in files {
        if let Some(backup) = &file.backup
            && let Err(error) = std::fs::remove_file(backup)
        {
            errors.push(format!(
                "failed to remove identity mirror backup {}: {error:#}",
                backup.display()
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(miette::miette!(errors.join("; ")))
    }
}

fn publish_error(message: String, files: &[PublishedFile]) -> miette::Report {
    let rollback_errors = rollback_published_identity_files(files);
    if rollback_errors.is_empty() {
        miette::miette!("{message}; identity mirror publish rollback completed")
    } else {
        miette::miette!(
            "{message}; identity mirror publish rollback failed: {}",
            rollback_errors.join("; ")
        )
    }
}

fn rollback_published_identity_files(files: &[PublishedFile]) -> Vec<String> {
    let mut errors = Vec::new();
    for file in files.iter().rev() {
        if file.published
            && let Err(error) = std::fs::remove_file(&file.destination)
        {
            errors.push(format!(
                "failed to remove newly published identity mirror {}: {error:#}",
                file.destination.display()
            ));
        }

        if let Some(backup) = &file.backup
            && let Err(error) = std::fs::rename(backup, &file.destination)
        {
            errors.push(format!(
                "failed to restore identity mirror backup {} to {}: {error:#}",
                backup.display(),
                file.destination.display()
            ));
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use right_openshell::test_support::{PROCESS_ENV_LOCK, PathGuard};
    use std::os::unix::fs::{PermissionsExt, symlink};

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
    fn publish_failure_restores_original_mirror_and_cleans_temporary_artifacts() {
        const ORIGINAL_CONTENTS: [&[u8]; 3] = [
            b"original identity\0\xff",
            b"original soul\nsecond line",
            b"original user\r\n",
        ];
        const NEW_CONTENTS: [&[u8]; 3] = [b"new identity", b"new soul", b"new user"];

        for fail_at in [1, 2] {
            let tmp = tempfile::tempdir().unwrap();
            let agent_dir = tmp.path().join("agent");
            std::fs::create_dir(&agent_dir).unwrap();
            for (filename, contents) in IDENTITY_MIRROR_FILES.into_iter().zip(ORIGINAL_CONTENTS) {
                std::fs::write(agent_dir.join(filename), contents).unwrap();
            }

            {
                let staging = tempfile::tempdir_in(&agent_dir).unwrap();
                for (filename, contents) in IDENTITY_MIRROR_FILES.into_iter().zip(NEW_CONTENTS) {
                    std::fs::write(staging.path().join(filename), contents).unwrap();
                }

                let mut publish_index = 0;
                let error = publish_staged_identity_mirror(
                    staging.path(),
                    &agent_dir,
                    |source, destination| {
                        let current_index = publish_index;
                        publish_index += 1;
                        if current_index == fail_at {
                            return Err(std::io::Error::other("simulated publish failure"));
                        }
                        std::fs::rename(source, destination)
                    },
                )
                .expect_err("a later publish failure must fail the transaction");

                let message = format!("{error:#}");
                assert!(message.contains("simulated publish failure"), "{message}");
                assert!(message.contains("publish rollback completed"), "{message}");
                for filename in IDENTITY_MIRROR_FILES {
                    assert!(
                        !staging.path().join(format!(".backup-{filename}")).exists(),
                        "rollback backup for {filename} must be restored"
                    );
                }
            }

            for (filename, expected) in IDENTITY_MIRROR_FILES.into_iter().zip(ORIGINAL_CONTENTS) {
                assert_eq!(
                    std::fs::read(agent_dir.join(filename)).unwrap(),
                    expected,
                    "failure at publish index {fail_at} changed {filename}"
                );
            }
            let mut entries = std::fs::read_dir(&agent_dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>();
            entries.sort();
            assert_eq!(
                entries,
                ["IDENTITY.md", "SOUL.md", "USER.md"]
                    .map(std::ffi::OsString::from)
                    .to_vec(),
                "temporary publish artifacts must be cleaned after failure at index {fail_at}"
            );
        }
    }
    #[test]
    fn publish_rejects_symlink_destination_before_mutating_any_mirror() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir(&agent_dir).unwrap();
        let symlink_target = agent_dir.join("user-target");
        std::fs::write(&symlink_target, "original user").unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "original first mirror").unwrap();
        std::fs::write(agent_dir.join("SOUL.md"), "original second mirror").unwrap();
        symlink(&symlink_target, agent_dir.join("USER.md")).unwrap();

        let staging = tempfile::tempdir_in(&agent_dir).unwrap();
        for filename in IDENTITY_MIRROR_FILES {
            std::fs::write(staging.path().join(filename), format!("new {filename}")).unwrap();
        }

        let mut publish_calls = 0;
        let error =
            publish_staged_identity_mirror(staging.path(), &agent_dir, |source, destination| {
                publish_calls += 1;
                std::fs::rename(source, destination)
            })
            .expect_err("a symlink destination must be rejected");

        assert!(format!("{error:#}").contains("is not a regular file"));
        assert_eq!(publish_calls, 0, "preflight must reject before publishing");
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("IDENTITY.md")).unwrap(),
            "original first mirror"
        );
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("SOUL.md")).unwrap(),
            "original second mirror"
        );
        let user_metadata = std::fs::symlink_metadata(agent_dir.join("USER.md")).unwrap();
        assert!(user_metadata.file_type().is_symlink());
        assert_eq!(
            std::fs::read_link(agent_dir.join("USER.md")).unwrap(),
            symlink_target
        );
        assert_eq!(
            std::fs::read_to_string(&symlink_target).unwrap(),
            "original user"
        );
        for filename in IDENTITY_MIRROR_FILES {
            assert!(!staging.path().join(format!(".backup-{filename}")).exists());
        }
    }

    #[test]
    fn successful_publish_removes_renamed_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir(&agent_dir).unwrap();
        let staging = tempfile::tempdir_in(&agent_dir).unwrap();
        for filename in IDENTITY_MIRROR_FILES {
            std::fs::write(agent_dir.join(filename), format!("old {filename}")).unwrap();
            std::fs::write(staging.path().join(filename), format!("new {filename}")).unwrap();
        }

        publish_staged_identity_mirror(staging.path(), &agent_dir, |source, destination| {
            std::fs::rename(source, destination)
        })
        .unwrap();

        for filename in IDENTITY_MIRROR_FILES {
            assert_eq!(
                std::fs::read_to_string(agent_dir.join(filename)).unwrap(),
                format!("new {filename}")
            );
            assert!(
                !staging.path().join(format!(".backup-{filename}")).exists(),
                "successful publish must clean backup for {filename}"
            );
        }
    }

    #[test]
    fn publish_failure_removes_files_without_originals() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir(&agent_dir).unwrap();
        let staging = tempfile::tempdir_in(&agent_dir).unwrap();
        for filename in IDENTITY_MIRROR_FILES {
            std::fs::write(staging.path().join(filename), format!("new {filename}")).unwrap();
        }

        let mut publish_index = 0;
        publish_staged_identity_mirror(staging.path(), &agent_dir, |source, destination| {
            let current_index = publish_index;
            publish_index += 1;
            if current_index == 1 {
                return Err(std::io::Error::other("simulated publish failure"));
            }
            std::fs::rename(source, destination)
        })
        .expect_err("the second publish must fail the transaction");

        for filename in IDENTITY_MIRROR_FILES {
            assert!(
                !agent_dir.join(filename).exists(),
                "rollback must remove newly published {filename}"
            );
        }
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
        for filename in IDENTITY_MIRROR_FILES {
            assert!(
                !agent_dir.join(filename).exists(),
                "failed reconciliation must not publish partial mirror file {filename}"
            );
        }
    }
}
