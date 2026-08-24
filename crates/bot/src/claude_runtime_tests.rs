use super::*;

#[test]
fn pinned_metadata_selects_linux_glibc_by_guest_arch() {
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
fn unsupported_guest_arch_is_explicit_error() {
    let error = artifact_spec_for_arch("riscv64").expect_err("unsupported arch must fail");
    assert!(
        error
            .to_string()
            .contains("unsupported Claude guest architecture")
    );
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
        let _ = stream
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
            .expect("write fixture headers");
        stream.write_all(&body).await.expect("write fixture body");
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn download_streams_verifies_and_cache_hit_needs_no_network() {
    let bytes = b"small fake Claude ELF".repeat(1024);
    let cache = tempfile::tempdir().expect("cache tempdir");
    let (base_url, server) = serve_once(bytes.clone()).await;
    let source = ArtifactSource {
        base_url,
        cache_root: cache.path().to_owned(),
        spec: fake_spec(&bytes),
    };
    let first = prepare_artifact_from(&source)
        .await
        .expect("initial download");
    server.await.expect("HTTP fixture task");
    assert_eq!(tokio::fs::read(&first).await.expect("read cache"), bytes);
    let offline_source = ArtifactSource {
        base_url: "http://127.0.0.1:1".to_owned(),
        ..source
    };
    let second = prepare_artifact_from(&offline_source)
        .await
        .expect("verified cache hit must avoid network");
    assert_eq!(second, first);
}

#[tokio::test]
async fn checksum_and_size_mismatches_publish_nothing_and_preserve_prior_entry() {
    for mismatch in ["checksum", "size"] {
        let bytes = b"downloaded bytes".repeat(64);
        let cache = tempfile::tempdir().expect("cache tempdir");
        let prior = cache.path().join("prior-working-runtime");
        tokio::fs::write(&prior, b"old runtime")
            .await
            .expect("seed prior runtime");
        let (base_url, server) = serve_once(bytes.clone()).await;
        let mut spec = fake_spec(&bytes);
        if mismatch == "checksum" {
            spec.sha256 = "00".repeat(32);
        } else {
            spec.size += 1;
        }
        let final_path = cache.path().join(spec.cache_name());
        let source = ArtifactSource {
            base_url,
            cache_root: cache.path().to_owned(),
            spec,
        };
        let error = prepare_artifact_from(&source)
            .await
            .expect_err("mismatched artifact must fail");
        server.await.expect("HTTP fixture task");
        assert!(error.to_string().contains("mismatch"), "{error:#}");
        assert!(!final_path.exists(), "mismatch published cache entry");
        assert_eq!(
            tokio::fs::read(&prior).await.expect("read prior runtime"),
            b"old runtime"
        );
        let entries = std::fs::read_dir(cache.path())
            .expect("list cache")
            .map(|entry| entry.expect("cache entry").file_name())
            .collect::<Vec<_>>();
        assert!(
            entries
                .iter()
                .all(|name| !name.to_string_lossy().ends_with(".download")),
            "partial download remains: {entries:?}"
        );
    }
}

#[tokio::test]
async fn corrupt_addressed_cache_entry_is_not_overwritten() {
    let bytes = b"expected runtime".repeat(32);
    let cache = tempfile::tempdir().expect("cache tempdir");
    let spec = fake_spec(&bytes);
    let final_path = cache.path().join(spec.cache_name());
    tokio::fs::write(&final_path, b"prior corrupt bytes")
        .await
        .expect("seed corrupt cache entry");
    let source = ArtifactSource {
        base_url: "http://127.0.0.1:1".to_owned(),
        cache_root: cache.path().to_owned(),
        spec,
    };

    let error = prepare_artifact_from(&source)
        .await
        .expect_err("corrupt addressed cache entry must fail closed");
    assert!(
        error.to_string().contains("refusing to overwrite"),
        "{error:#}"
    );
    assert_eq!(
        tokio::fs::read(&final_path)
            .await
            .expect("read corrupt entry"),
        b"prior corrupt bytes"
    );
}
#[test]
fn guest_activation_is_content_addressed_and_atomic_after_verification() {
    let spec = fake_spec(b"fake Claude");
    let plan = guest_activation_plan(&spec, "nonce");
    assert_eq!(
        plan.target,
        format!(
            "/sandbox/.platform/claude/claude-test-version-linux-test-{}",
            spec.sha256
        )
    );
    assert_eq!(plan.upload, "/sandbox/.platform/claude/.upload-nonce");
    assert_eq!(plan.chmod.cmd, "chmod");
    assert_eq!(plan.chmod.args, ["0555", plan.upload.as_str()]);
    assert_eq!(plan.hash.cmd, "sha256sum");
    assert_eq!(plan.publish.cmd, "mv");
    assert_eq!(plan.create_link.cmd, "ln");
    assert_eq!(plan.activate_link.cmd, "mv");
    assert_eq!(
        plan.activate_link.args.last().map(String::as_str),
        Some(PLATFORM_CLAUDE_LINK)
    );
    for request in [
        &plan.chmod,
        &plan.hash,
        &plan.publish,
        &plan.create_link,
        &plan.activate_link,
    ] {
        assert_eq!(
            request.user, None,
            "activation request must run as SDK root"
        );
        assert!(
            request
                .args
                .iter()
                .filter(|arg| arg.starts_with('/'))
                .all(|arg| arg.starts_with(PLATFORM_ROOT)),
            "root request escaped fixed platform root: {request:?}"
        );
    }
}
