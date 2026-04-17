use super::*;
use tempfile::TempDir;

fn t() -> DateTime<Utc> {
    "2026-04-16T12:00:00Z".parse().unwrap()
}

mod parse_serialize_tests {
    use super::*;

    #[test]
    fn parses_empty_allowlist() {
        let text = "version: 1\nusers: []\ngroups: []\n";
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.version, 1);
        assert!(parsed.users.is_empty());
        assert!(parsed.groups.is_empty());
    }

    #[test]
    fn parses_users_and_groups() {
        let text = r#"version: 1
users:
  - id: 42
    label: andrey
    added_by: null
    added_at: 2026-04-16T12:00:00Z
groups:
  - id: -1001
    label: "Dev Team"
    opened_by: 42
    opened_at: 2026-04-16T12:30:00Z
"#;
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.users.len(), 1);
        assert_eq!(parsed.users[0].id, 42);
        assert_eq!(parsed.users[0].label.as_deref(), Some("andrey"));
        assert_eq!(parsed.users[0].added_by, None);
        assert_eq!(parsed.groups.len(), 1);
        assert_eq!(parsed.groups[0].id, -1001);
        assert_eq!(parsed.groups[0].opened_by, Some(42));
    }

    #[test]
    fn missing_version_defaults_to_1() {
        let text = "users: []\ngroups: []\n";
        let parsed = parse_yaml(text).unwrap();
        assert_eq!(parsed.version, 1);
    }

    #[test]
    fn rejects_unknown_version() {
        let text = "version: 99\nusers: []\ngroups: []\n";
        let err = parse_yaml(text).unwrap_err();
        assert!(err.contains("version 99"));
    }

    #[test]
    fn serialize_roundtrip() {
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 42,
                label: Some("andrey".into()),
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![AllowedGroup {
                id: -1001,
                label: Some("Dev Team".into()),
                opened_by: Some(42),
                opened_at: "2026-04-16T12:30:00Z".parse().unwrap(),
            }],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed, file);
    }

    #[test]
    fn serialize_empty_lists_use_flow_style() {
        let file = AllowlistFile::default();
        let yaml = serialize_yaml(&file);
        assert!(yaml.contains("users: []"));
        assert!(yaml.contains("groups: []"));
    }

    #[test]
    fn serialize_escapes_quotes_in_label() {
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 1,
                label: Some(r#"has "quotes""#.into()),
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed.users[0].label.as_deref(), Some(r#"has "quotes""#));
    }

    #[test]
    fn allowed_user_rejects_unknown_fields() {
        let yaml = r#"
id: 42
opned_by: 7
added_at: 2026-04-16T12:00:00Z
"#;
        let result: Result<AllowedUser, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown field"),
            "expected 'unknown field' in error: {err}"
        );
    }

    #[test]
    fn allowed_group_rejects_unknown_fields() {
        let yaml = r#"
id: -1001
opend_by: 7
opened_at: 2026-04-16T12:00:00Z
"#;
        let result: Result<AllowedGroup, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown field"),
            "expected 'unknown field' in error: {err}"
        );
    }

    #[test]
    fn allowlist_file_rejects_unknown_fields() {
        let yaml = r#"
version: 1
usrs: []
groups: []
"#;
        let result: Result<AllowlistFile, _> = serde_saphyr::from_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unknown field"),
            "expected 'unknown field' in error: {err}"
        );
    }

    #[test]
    fn serialize_escapes_control_chars_in_label() {
        // Literal backslash, double quote, newline, carriage return, tab,
        // NUL, \x05 (arbitrary control char), plus non-ASCII (emoji + Cyrillic).
        let tricky = "a\\b\"c\nd\re\tf\0g\x05h \u{1F600} \u{0410}\u{0411}";
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 1,
                label: Some(tricky.to_string()),
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed.users[0].label.as_deref(), Some(tricky));
    }

    #[test]
    fn serialize_null_label_roundtrips_as_none() {
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 1,
                label: None,
                added_by: None,
                added_at: "2026-04-16T12:00:00Z".parse().unwrap(),
            }],
            groups: vec![AllowedGroup {
                id: -1001,
                label: None,
                opened_by: None,
                opened_at: "2026-04-16T12:30:00Z".parse().unwrap(),
            }],
        };
        let yaml = serialize_yaml(&file);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(parsed.users[0].label, None);
        assert_eq!(parsed.groups[0].label, None);
    }
}

mod state_tests {
    use super::*;

    #[test]
    fn add_user_inserted_then_already_present() {
        let mut s = AllowlistState::default();
        let u = AllowedUser { id: 1, label: None, added_by: None, added_at: t() };
        assert_eq!(s.add_user(u.clone()), AddOutcome::Inserted);
        assert_eq!(s.add_user(u), AddOutcome::AlreadyPresent);
        assert_eq!(s.users().len(), 1);
    }

    #[test]
    fn remove_user_removed_then_not_found() {
        let mut s = AllowlistState::default();
        let u = AllowedUser { id: 1, label: None, added_by: None, added_at: t() };
        s.add_user(u);
        assert_eq!(s.remove_user(1), RemoveOutcome::Removed);
        assert_eq!(s.remove_user(1), RemoveOutcome::NotFound);
    }

    #[test]
    fn is_user_trusted_reflects_state() {
        let mut s = AllowlistState::default();
        assert!(!s.is_user_trusted(99));
        s.add_user(AllowedUser { id: 99, label: None, added_by: None, added_at: t() });
        assert!(s.is_user_trusted(99));
    }

    #[test]
    fn add_group_and_is_open() {
        let mut s = AllowlistState::default();
        assert!(!s.is_group_open(-1));
        s.add_group(AllowedGroup { id: -1, label: None, opened_by: Some(1), opened_at: t() });
        assert!(s.is_group_open(-1));
    }

    #[tokio::test]
    async fn handle_is_shareable_across_tasks() {
        let h = AllowlistHandle::new(AllowlistState::default());
        let h2 = h.clone();
        tokio::spawn(async move {
            let mut w = h2.0.write().unwrap();
            w.add_user(AllowedUser { id: 7, label: None, added_by: None, added_at: t() });
        })
        .await
        .unwrap();
        let r = h.0.read().unwrap();
        assert!(r.is_user_trusted(7));
    }
}

mod io_tests {
    use super::*;

    #[test]
    fn read_nonexistent_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_file(dir.path()).unwrap().is_none());
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = TempDir::new().unwrap();
        let file = AllowlistFile {
            version: 1,
            users: vec![AllowedUser {
                id: 1,
                label: Some("u".into()),
                added_by: None,
                added_at: t(),
            }],
            groups: vec![AllowedGroup {
                id: -1,
                label: None,
                opened_by: Some(1),
                opened_at: t(),
            }],
        };
        write_file(dir.path(), &file).unwrap();
        let read = read_file(dir.path()).unwrap().unwrap();
        assert_eq!(read, file);
    }

    #[test]
    fn atomic_write_leaves_no_tmp_file() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), &AllowlistFile::default()).unwrap();
        let tmp = dir.path().join("allowlist.yaml.tmp");
        assert!(!tmp.exists(), "tmp file must be renamed away");
        assert!(dir.path().join("allowlist.yaml").exists());
    }

    #[test]
    fn with_lock_creates_lockfile_and_returns_result() {
        let dir = TempDir::new().unwrap();
        let r = with_lock(dir.path(), |_| Ok::<_, String>(42)).unwrap();
        assert_eq!(r, 42);
        assert!(dir.path().join("allowlist.yaml.lock").exists());
    }
}

mod migration_tests {
    use super::*;

    #[test]
    fn migration_splits_by_sign() {
        let dir = TempDir::new().unwrap();
        let report = migrate_from_legacy(dir.path(), &[42, -1001, 100, -500, 0], t()).unwrap();
        assert_eq!(report.migrated_users, 2);
        assert_eq!(report.migrated_groups, 2);
        assert!(!report.already_present);
        let file = read_file(dir.path()).unwrap().unwrap();
        let user_ids: Vec<i64> = file.users.iter().map(|u| u.id).collect();
        let group_ids: Vec<i64> = file.groups.iter().map(|g| g.id).collect();
        assert_eq!(user_ids, vec![42, 100]);
        assert_eq!(group_ids, vec![-1001, -500]);
        assert!(file.users.iter().all(|u| u.added_by.is_none()));
        assert!(file.groups.iter().all(|g| g.opened_by.is_none()));
    }

    #[test]
    fn migration_skips_when_allowlist_exists() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), &AllowlistFile::default()).unwrap();
        let report = migrate_from_legacy(dir.path(), &[42], t()).unwrap();
        assert!(report.already_present);
        assert_eq!(report.migrated_users, 0);
        let file = read_file(dir.path()).unwrap().unwrap();
        assert!(file.users.is_empty(), "should not overwrite existing file");
    }

    #[test]
    fn migration_empty_list_creates_empty_allowlist() {
        let dir = TempDir::new().unwrap();
        let report = migrate_from_legacy(dir.path(), &[], t()).unwrap();
        assert_eq!(report.migrated_users, 0);
        assert_eq!(report.migrated_groups, 0);
        assert!(!report.already_present);
        let file = read_file(dir.path()).unwrap().unwrap();
        assert!(file.users.is_empty());
        assert!(file.groups.is_empty());
    }
}

mod watcher_tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn watcher_reloads_after_external_write() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), &AllowlistFile::default()).unwrap();
        let handle = AllowlistHandle::new(AllowlistState::from_file(
            read_file(dir.path()).unwrap().unwrap(),
        ));
        let _watcher = spawn_watcher(dir.path(), handle.clone()).unwrap();

        // Externally mutate the file.
        let mut file = AllowlistFile::default();
        file.users.push(AllowedUser { id: 777, label: None, added_by: None, added_at: t() });
        write_file(dir.path(), &file).unwrap();

        // Poll up to 2s for the handle to reflect the change.
        for _ in 0..40 {
            {
                let r = handle.0.read().unwrap();
                if r.is_user_trusted(777) { return; }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("watcher did not propagate external write within 2s");
    }
}
