use super::*;

#[test]
fn pinned_metadata_selects_host_linux_guest_arch() {
    let arm64 = artifact_spec_for_arch("aarch64").expect("arm64 metadata");
    assert_eq!(arm64.version, CLAUDE_VERSION);
    assert_eq!(arm64.platform, "linux-arm64");
    assert_eq!(
        arm64.sha256,
        "2db0cb893ebed8ef8aee46656da45bc6801fa2586293dae64abfa3ade894a2fe"
    );
    assert_eq!(arm64.size, 339_794_152);
    let x64 = artifact_spec_for_arch("x86_64").expect("x64 metadata");
    assert_eq!(x64.platform, "linux-x64");
    assert_eq!(
        x64.sha256,
        "0771bd866cff82b76581fc0499f6529e1a36845078f144f8c81dccb3bc7037b8"
    );
    assert_eq!(x64.size, 342_636_848);
}

#[test]
fn unsupported_host_guest_arch_is_explicit_hard_error() {
    let error = artifact_spec_for_arch("riscv64").expect_err("unsupported arch must fail");
    assert!(format!("{error:#}").contains("Platform::host_linux"));
}

#[test]
fn runtime_paths_are_fixed_root_owned_and_sandbox_platform_is_irrelevant() {
    assert_eq!(CLAUDE_ROOT, "/opt/right");
    assert_eq!(CLAUDE_RUNTIME_DIR, "/opt/right/claude");
    assert_eq!(CLAUDE_BIN_DIR, "/opt/right/bin");
    assert_eq!(CLAUDE_ACTIVE_LINK, "/opt/right/bin/claude");
    for path in [
        CLAUDE_ROOT,
        CLAUDE_RUNTIME_DIR,
        CLAUDE_BIN_DIR,
        CLAUDE_ACTIVE_LINK,
    ] {
        assert!(!path.starts_with("/sandbox"));
    }
    let source = include_str!("claude_runtime.rs");
    assert!(!source.contains("/sandbox/.platform"));
}

fn fake_spec(bytes: &[u8]) -> ArtifactSpec {
    ArtifactSpec {
        version: "test-version".to_owned(),
        platform: "linux-test".to_owned(),
        sha256: digest_hex(&Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

async fn serve_once(body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind HTTP fixture");
    let address = listener.local_addr().expect("fixture address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept fixture request");
        let mut request = vec![0_u8; 4096];
        stream
            .read(&mut request)
            .await
            .expect("read fixture request");
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(headers.as_bytes())
            .await
            .expect("write headers");
        stream.write_all(&body).await.expect("write body");
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn download_verifies_cache_hit_and_publishes_read_only_file() {
    use std::os::unix::fs::PermissionsExt as _;
    let bytes = b"small fake Claude ELF".repeat(1024);
    let cache = tempfile::tempdir().expect("cache tempdir");
    let cache_root = cache.path().join("claude-code");
    let (base_url, server) = serve_once(bytes.clone()).await;
    let source = ArtifactSource {
        base_url,
        cache_root,
        spec: fake_spec(&bytes),
    };
    let first = prepare_artifact_from(&source)
        .await
        .expect("initial download");
    server.await.expect("HTTP fixture task");
    assert_eq!(tokio::fs::read(&first).await.expect("read cache"), bytes);
    assert_eq!(
        std::fs::metadata(&source.cache_root)
            .expect("cache metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&first)
            .expect("artifact metadata")
            .permissions()
            .mode()
            & 0o777,
        0o555
    );
    let second = prepare_artifact_from(&ArtifactSource {
        base_url: "http://127.0.0.1:1".to_owned(),
        ..source
    })
    .await
    .expect("offline verified hit");
    assert_eq!(second, first);
}

#[tokio::test]
async fn checksum_mismatch_cleans_partial_and_preserves_prior_runtime() {
    let bytes = b"downloaded bytes".repeat(64);
    let cache = tempfile::tempdir().expect("cache tempdir");
    let cache_root = cache.path().join("claude-code");
    tokio::fs::create_dir(&cache_root)
        .await
        .expect("create cache");
    let prior = cache_root.join("prior-working-runtime");
    tokio::fs::write(&prior, b"old runtime")
        .await
        .expect("seed prior");
    let (base_url, server) = serve_once(bytes.clone()).await;
    let mut spec = fake_spec(&bytes);
    spec.sha256 = "00".repeat(32);
    let final_path = cache_root.join(spec.cache_name());
    let source = ArtifactSource {
        base_url,
        cache_root: cache_root.clone(),
        spec,
    };
    let error = prepare_artifact_from(&source)
        .await
        .expect_err("mismatch must fail");
    server.await.expect("HTTP fixture task");
    assert!(format!("{error:#}").contains("mismatch"));
    assert!(!final_path.exists());
    assert_eq!(
        tokio::fs::read(&prior).await.expect("read prior"),
        b"old runtime"
    );
    let entries = std::fs::read_dir(cache_root)
        .expect("list cache")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert!(
        entries
            .iter()
            .all(|name| !name.to_string_lossy().ends_with(".download"))
    );
}

#[tokio::test]
async fn corrupt_regular_cache_entry_is_removed_and_redownloaded() {
    let bytes = b"expected runtime".repeat(32);
    let cache = tempfile::tempdir().expect("cache tempdir");
    let cache_root = cache.path().join("claude-code");
    tokio::fs::create_dir(&cache_root)
        .await
        .expect("create cache");
    let spec = fake_spec(&bytes);
    let final_path = cache_root.join(spec.cache_name());
    tokio::fs::write(&final_path, b"corrupt")
        .await
        .expect("seed corrupt");
    let (base_url, server) = serve_once(bytes.clone()).await;
    prepare_artifact_from(&ArtifactSource {
        base_url,
        cache_root,
        spec,
    })
    .await
    .expect("repair cache");
    server.await.expect("fixture");
    assert_eq!(
        tokio::fs::read(final_path).await.expect("read repaired"),
        bytes
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cache_root_lock_and_final_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;
    let outer = tempfile::tempdir().expect("outer");
    let real = outer.path().join("real");
    std::fs::create_dir(&real).expect("real cache");
    let root_link = outer.path().join("cache-link");
    symlink(&real, &root_link).expect("cache root link");
    let source = ArtifactSource {
        base_url: "http://127.0.0.1:1".to_owned(),
        cache_root: root_link,
        spec: fake_spec(b"x"),
    };
    assert!(
        prepare_artifact_from(&source)
            .await
            .expect_err("root symlink")
            .to_string()
            .contains("not a regular directory")
    );

    let cache_root = outer.path().join("cache");
    std::fs::create_dir(&cache_root).expect("cache");
    symlink(&real, cache_root.join(".download.lock")).expect("lock link");
    let source = ArtifactSource {
        cache_root: cache_root.clone(),
        ..source
    };
    assert!(
        prepare_artifact_from(&source)
            .await
            .expect_err("lock symlink")
            .to_string()
            .contains("lock")
    );

    std::fs::remove_file(cache_root.join(".download.lock")).expect("remove lock link");
    symlink(&real, cache_root.join(source.spec.cache_name())).expect("final link");
    assert!(
        prepare_artifact_from(&source)
            .await
            .expect_err("final symlink")
            .to_string()
            .contains("not a regular file")
    );
}

#[tokio::test]
async fn checksum_and_cache_layout_failures_are_hard_but_network_is_retryable() {
    let outer = tempfile::tempdir().expect("outer");
    let cache_root = outer.path().join("cache");
    std::fs::create_dir(&cache_root).expect("cache");
    let bytes = b"wrong checksum".repeat(8);
    let (base_url, server) = serve_once(bytes.clone()).await;
    let mut spec = fake_spec(&bytes);
    spec.sha256 = "00".repeat(32);
    let error = prepare_artifact_from_typed(&ArtifactSource {
        base_url,
        cache_root: cache_root.clone(),
        spec,
    })
    .await
    .expect_err("checksum is hard");
    server.await.expect("fixture");
    assert!(!error.is_retryable());

    let network = prepare_artifact_from_typed(&ArtifactSource {
        base_url: "http://127.0.0.1:1".to_owned(),
        cache_root,
        spec: fake_spec(b"network"),
    })
    .await
    .expect_err("network unavailable");
    assert!(network.is_retryable());
}

#[test]
fn root_directory_validation_rejects_symlink_and_writable_modes() {
    let source = include_str!("claude_runtime.rs");
    assert!(source.contains("root_request(\"test\", &[\"!\", \"-L\", path])"));
    assert!(source.contains("mode & 0o022 != 0"));
    assert!(source.contains("fields.get(1) != Some(&\"0\")"));
    assert!(source.contains("fields.get(2) != Some(&\"0\")"));
}

#[test]
fn post_activation_gc_is_nonfatal_and_mtime_order_is_numeric() {
    let source = include_str!("claude_runtime.rs");
    assert!(source.contains("mtime.parse::<f64>()"));
    assert!(source.contains("if let Err(error) = gc_guest_versions"));
}

#[test]
fn http_client_has_connect_and_total_timeout_contract() {
    let source = include_str!("claude_runtime.rs");
    assert!(source.contains(".connect_timeout(HTTP_CONNECT_TIMEOUT)"));
    assert!(source.contains(".timeout(HTTP_TOTAL_TIMEOUT)"));
}

#[test]
fn acquisition_retryability_is_typed_but_arch_and_layout_are_hard() {
    let transient = ClaudeRuntimeError::Retryable(miette::miette!("network"));
    let hard = ClaudeRuntimeError::Hard(miette::miette!("invalid arch"));
    assert!(transient.is_retryable());
    assert!(!hard.is_retryable());
}

#[test]
fn repeat_activation_uses_link_aware_fixed_argv_not_following_fs_stat() {
    let source = include_str!("claude_runtime.rs");
    assert!(source.contains("root_request(\"test\", &[\"-L\", CLAUDE_ACTIVE_LINK])"));
    assert!(!source.contains("fs_stat(CLAUDE_ACTIVE_LINK)"));
}

#[test]
fn guest_activation_is_atomic_only_after_hash_and_mode_verification() {
    let spec = fake_spec(b"fake Claude");
    let plan = guest_activation_plan(&spec, "nonce");
    assert_eq!(
        plan.target,
        format!(
            "/opt/right/claude/claude-test-version-linux-test-{}",
            spec.sha256
        )
    );
    assert_eq!(plan.upload, "/opt/right/claude/.upload-nonce");
    assert_eq!(plan.temporary_link, "/opt/right/bin/.claude-link-nonce");
    assert_eq!(plan.chmod.args, ["0555", plan.upload.as_str()]);
    assert_eq!(plan.hash.cmd, "sha256sum");
    assert_eq!(plan.publish.cmd, "mv");
    assert_eq!(plan.create_link.cmd, "ln");
    assert_eq!(
        plan.activate_link.args.last().map(String::as_str),
        Some(CLAUDE_ACTIVE_LINK)
    );
    for request in [
        &plan.chmod,
        &plan.hash,
        &plan.publish,
        &plan.create_link,
        &plan.activate_link,
    ] {
        assert_eq!(request.user, None);
        assert!(
            request
                .args
                .iter()
                .filter(|arg| arg.starts_with('/'))
                .all(|arg| arg.starts_with("/opt/right"))
        );
    }
}

#[test]
fn active_link_is_not_replaced_until_upload_hash_and_mode_succeed() {
    let source = include_str!("claude_runtime.rs");
    let upload = source.find("fs_copy_from_host").expect("upload");
    let hash = source[upload..].find("plan.hash").expect("hash") + upload;
    let mode = source[hash..].find("stat").expect("mode stat") + hash;
    let activate = source[mode..].find("plan.activate_link").expect("activate") + mode;
    assert!(upload < hash && hash < mode && mode < activate);
    assert!(source.contains("cleanup_guest_path(sandbox, &plan.upload).await"));
    assert!(source.contains("cleanup_guest_path(sandbox, &plan.temporary_link).await"));
}
