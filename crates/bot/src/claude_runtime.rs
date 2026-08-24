//! Host-side acquisition and guest activation of the pinned Claude Code runtime.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::FileExt;
use futures::StreamExt as _;
use right_sandbox::{ExecRequest, FsEntryKind};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::sandbox::Sandbox;

const CLAUDE_VERSION: &str = "2.1.241";
const ARTIFACT_BASE_URL: &str = "https://downloads.claude.ai/claude-code-releases";
const PLATFORM_ROOT: &str = "/sandbox/.platform";
const PLATFORM_CLAUDE_DIR: &str = "/sandbox/.platform/claude";
const PLATFORM_BIN_DIR: &str = "/sandbox/.platform/bin";
const PLATFORM_CLAUDE_LINK: &str = "/sandbox/.platform/bin/claude";
const GUEST_OPERATION_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactSpec {
    version: String,
    platform: String,
    sha256: String,
    size: u64,
}

impl ArtifactSpec {
    fn cache_name(&self) -> String {
        format!("claude-{}-{}-{}", self.version, self.platform, self.sha256)
    }

    fn guest_target(&self) -> String {
        format!("{PLATFORM_CLAUDE_DIR}/{}", self.cache_name())
    }
}

#[derive(Debug, Clone)]
struct ArtifactSource {
    base_url: String,
    cache_root: PathBuf,
    spec: ArtifactSpec,
}

fn artifact_spec_for_arch(arch: &str) -> miette::Result<ArtifactSpec> {
    let (platform, sha256, size) = match arch {
        "aarch64" | "arm64" => (
            "linux-arm64",
            "2db0cb893ebed8ef8aee46656da45bc6801fa2586293dae64abfa3ade894a2fe",
            339_794_152,
        ),
        "x86_64" | "amd64" => (
            "linux-x64",
            "0771bd866cff82b76581fc0499f6529e1a36845078f144f8c81dccb3bc7037b8",
            342_636_848,
        ),
        unsupported => {
            miette::bail!("unsupported Claude guest architecture: {unsupported}");
        }
    };
    Ok(ArtifactSpec {
        version: CLAUDE_VERSION.to_owned(),
        platform: platform.to_owned(),
        sha256: sha256.to_owned(),
        size,
    })
}

fn cache_root_for_agent(agent_dir: &Path) -> miette::Result<PathBuf> {
    let agents_dir = agent_dir.parent().ok_or_else(|| {
        miette::miette!(
            "agent directory has no agents parent: {}",
            agent_dir.display()
        )
    })?;
    let right_home = agents_dir.parent().ok_or_else(|| {
        miette::miette!(
            "agent directory has no Right home ancestor: {}",
            agent_dir.display()
        )
    })?;
    Ok(right_home.join("cache/claude-code"))
}

async fn prepare_artifact(agent_dir: &Path) -> miette::Result<(PathBuf, ArtifactSpec)> {
    let source = ArtifactSource {
        base_url: ARTIFACT_BASE_URL.to_owned(),
        cache_root: cache_root_for_agent(agent_dir)?,
        spec: artifact_spec_for_arch(std::env::consts::ARCH)?,
    };
    let path = prepare_artifact_from(&source).await?;
    Ok((path, source.spec))
}

async fn prepare_artifact_from(source: &ArtifactSource) -> miette::Result<PathBuf> {
    tokio::fs::create_dir_all(&source.cache_root)
        .await
        .map_err(|e| {
            miette::miette!(
                "create Claude artifact cache {}: {e:#}",
                source.cache_root.display()
            )
        })?;

    let lock_path = source.cache_root.join(".download.lock");
    let lock = tokio::task::spawn_blocking(move || -> miette::Result<std::fs::File> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| {
                miette::miette!("open Claude cache lock {}: {e:#}", lock_path.display())
            })?;
        FileExt::lock(&file).map_err(|e| {
            miette::miette!("acquire Claude cache lock {}: {e:#}", lock_path.display())
        })?;
        Ok(file)
    })
    .await
    .map_err(|e| miette::miette!("join Claude cache lock task: {e:#}"))??;

    let result = prepare_artifact_locked(source).await;
    FileExt::unlock(&lock).map_err(|e| miette::miette!("release Claude cache lock: {e:#}"))?;
    result
}

async fn prepare_artifact_locked(source: &ArtifactSource) -> miette::Result<PathBuf> {
    let final_path = source.cache_root.join(source.spec.cache_name());
    if tokio::fs::try_exists(&final_path)
        .await
        .map_err(|error| miette::miette!("check cached Claude artifact: {error:#}"))?
    {
        if artifact_matches(&final_path, &source.spec).await? {
            return Ok(final_path);
        }
        miette::bail!(
            "cached Claude artifact failed verification at {}; refusing to overwrite it",
            final_path.display()
        );
    }

    let temp_path = source.cache_root.join(format!(
        ".{}.{}.download",
        source.spec.cache_name(),
        uuid::Uuid::new_v4()
    ));
    let result = download_verified(source, &temp_path).await;
    if let Err(error) = result {
        let cleanup = tokio::fs::remove_file(&temp_path).await;
        if let Err(cleanup_error) = cleanup
            && cleanup_error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.wrap_err(format!(
                "also failed to remove incomplete download {}: {cleanup_error:#}",
                temp_path.display()
            )));
        }
        return Err(error);
    }
    tokio::fs::rename(&temp_path, &final_path)
        .await
        .map_err(|e| {
            miette::miette!(
                "publish verified Claude artifact {}: {e:#}",
                final_path.display()
            )
        })?;
    Ok(final_path)
}

async fn artifact_matches(path: &Path, spec: &ArtifactSpec) -> miette::Result<bool> {
    let metadata = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(miette::miette!(
                "stat cached Claude artifact {}: {error:#}",
                path.display()
            ));
        }
    };
    if !metadata.is_file() || metadata.len() != spec.size {
        return Ok(false);
    }
    let (size, digest) = hash_file(path).await?;
    Ok(size == spec.size && digest == spec.sha256)
}

async fn hash_file(path: &Path) -> miette::Result<(u64, String)> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| miette::miette!("open artifact {} for hashing: {e:#}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let count = tokio::io::AsyncReadExt::read(&mut file, &mut buffer)
            .await
            .map_err(|e| miette::miette!("read artifact {}: {e:#}", path.display()))?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hasher.update(&buffer[..count]);
    }
    Ok((size, digest_hex(&hasher.finalize())))
}

fn digest_hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

async fn download_verified(source: &ArtifactSource, destination: &Path) -> miette::Result<()> {
    let url = format!(
        "{}/{}/{}/claude",
        source.base_url.trim_end_matches('/'),
        source.spec.version,
        source.spec.platform
    );
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| miette::miette!("download pinned Claude artifact {url}: {e:#}"))?
        .error_for_status()
        .map_err(|e| miette::miette!("download pinned Claude artifact {url}: {e:#}"))?;
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .await
        .map_err(|e| {
            miette::miette!(
                "create Claude artifact download {}: {e:#}",
                destination.display()
            )
        })?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| miette::miette!("stream Claude artifact {url}: {e:#}"))?;
        size += chunk.len() as u64;
        if size > source.spec.size {
            miette::bail!(
                "Claude artifact size mismatch: expected {}, received more than {} bytes",
                source.spec.size,
                source.spec.size
            );
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| miette::miette!("write Claude artifact download: {e:#}"))?;
    }
    file.flush()
        .await
        .map_err(|e| miette::miette!("flush Claude artifact download: {e:#}"))?;
    let digest = digest_hex(&hasher.finalize());
    if size != source.spec.size {
        miette::bail!(
            "Claude artifact size mismatch: expected {}, received {size} bytes",
            source.spec.size
        );
    }
    if digest != source.spec.sha256 {
        miette::bail!(
            "Claude artifact SHA-256 mismatch: expected {}, received {digest}",
            source.spec.sha256
        );
    }
    file.sync_all()
        .await
        .map_err(|e| miette::miette!("sync verified Claude artifact download: {e:#}"))?;
    Ok(())
}

#[derive(Debug)]
struct GuestActivationPlan {
    target: String,
    upload: String,
    chmod: ExecRequest,
    hash: ExecRequest,
    publish: ExecRequest,
    create_link: ExecRequest,
    activate_link: ExecRequest,
}

fn root_request(cmd: &str, args: &[&str]) -> ExecRequest {
    ExecRequest {
        cmd: cmd.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        timeout: Some(GUEST_OPERATION_TIMEOUT),
        ..ExecRequest::default()
    }
}

fn guest_activation_plan(spec: &ArtifactSpec, nonce: &str) -> GuestActivationPlan {
    let target = spec.guest_target();
    let upload = format!("{PLATFORM_CLAUDE_DIR}/.upload-{nonce}");
    let temporary_link = format!("{PLATFORM_BIN_DIR}/.claude-link-{nonce}");
    GuestActivationPlan {
        chmod: root_request("chmod", &["0555", &upload]),
        hash: root_request("sha256sum", &[&upload]),
        publish: root_request("mv", &["-fT", &upload, &target]),
        create_link: root_request("ln", &["-s", &target, &temporary_link]),
        activate_link: root_request("mv", &["-fT", &temporary_link, PLATFORM_CLAUDE_LINK]),
        target,
        upload,
    }
}

async fn ensure_platform_directory(sandbox: &Sandbox, path: &str) -> miette::Result<()> {
    if sandbox
        .fs_exists(path)
        .await
        .map_err(|e| miette::miette!("check platform directory {path}: {e:#}"))?
    {
        let stat = sandbox
            .fs_stat(path)
            .await
            .map_err(|e| miette::miette!("stat platform directory {path}: {e:#}"))?;
        if stat.kind != FsEntryKind::Directory || stat.uid != 0 {
            miette::bail!(
                "refusing unexpected platform path {path}: kind={:?}, uid={}",
                stat.kind,
                stat.uid
            );
        }
        return Ok(());
    }
    sandbox
        .fs_mkdir(path)
        .await
        .map_err(|e| miette::miette!("create platform directory {path}: {e:#}"))
}

async fn run_root_request(sandbox: &Sandbox, request: &ExecRequest) -> miette::Result<String> {
    let outcome = sandbox.exec(request).await.map_err(|e| {
        miette::miette!(
            "run platform-owned guest command {} {:?}: {e:#}",
            request.cmd,
            request.args
        )
    })?;
    if outcome.code != 0 {
        miette::bail!(
            "platform-owned guest command {} {:?} exited {}: {}{}",
            request.cmd,
            request.args,
            outcome.code,
            String::from_utf8_lossy(&outcome.stdout),
            String::from_utf8_lossy(&outcome.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&outcome.stdout).into_owned())
}

async fn guest_artifact_matches(
    sandbox: &Sandbox,
    path: &str,
    spec: &ArtifactSpec,
) -> miette::Result<bool> {
    if !sandbox
        .fs_exists(path)
        .await
        .map_err(|e| miette::miette!("check guest Claude artifact {path}: {e:#}"))?
    {
        return Ok(false);
    }
    let stat = sandbox
        .fs_stat(path)
        .await
        .map_err(|e| miette::miette!("stat guest Claude artifact {path}: {e:#}"))?;
    if stat.kind != FsEntryKind::File || stat.uid != 0 || stat.size != spec.size {
        return Ok(false);
    }
    let output = run_root_request(sandbox, &root_request("sha256sum", &[path])).await?;
    Ok(output.split_whitespace().next() == Some(spec.sha256.as_str()))
}

async fn activate_artifact(
    sandbox: &Sandbox,
    artifact_path: &Path,
    spec: &ArtifactSpec,
) -> miette::Result<()> {
    ensure_platform_directory(sandbox, PLATFORM_ROOT).await?;
    ensure_platform_directory(sandbox, PLATFORM_CLAUDE_DIR).await?;
    ensure_platform_directory(sandbox, PLATFORM_BIN_DIR).await?;

    if sandbox
        .fs_exists(PLATFORM_CLAUDE_LINK)
        .await
        .map_err(|e| miette::miette!("check active Claude platform link: {e:#}"))?
    {
        let stat = sandbox
            .fs_stat(PLATFORM_CLAUDE_LINK)
            .await
            .map_err(|e| miette::miette!("stat active Claude platform link: {e:#}"))?;
        if stat.kind != FsEntryKind::Symlink {
            miette::bail!("refusing non-symlink active Claude path {PLATFORM_CLAUDE_LINK}");
        }
    }

    let plan = guest_activation_plan(spec, &uuid::Uuid::new_v4().simple().to_string());
    if !guest_artifact_matches(sandbox, &plan.target, spec).await? {
        sandbox
            .fs_copy_from_host(artifact_path, &plan.upload)
            .await
            .map_err(|e| {
                miette::miette!(
                    "upload verified Claude artifact {} -> {}: {e:#}",
                    artifact_path.display(),
                    plan.upload
                )
            })?;
        run_root_request(sandbox, &plan.chmod).await?;
        let hash_output = run_root_request(sandbox, &plan.hash).await?;
        if hash_output.split_whitespace().next() != Some(spec.sha256.as_str()) {
            miette::bail!("guest Claude artifact SHA-256 mismatch after upload");
        }
        let stat = sandbox
            .fs_stat(&plan.upload)
            .await
            .map_err(|e| miette::miette!("stat uploaded Claude artifact: {e:#}"))?;
        if stat.kind != FsEntryKind::File || stat.uid != 0 || stat.size != spec.size {
            miette::bail!(
                "guest Claude artifact metadata mismatch after upload: kind={:?}, uid={}, size={}",
                stat.kind,
                stat.uid,
                stat.size
            );
        }
        run_root_request(sandbox, &plan.publish).await?;
    }
    run_root_request(sandbox, &plan.create_link).await?;
    run_root_request(sandbox, &plan.activate_link).await?;
    tracing::info!(
        version = spec.version,
        platform = spec.platform,
        target = plan.target,
        "sync: activated host-staged Claude runtime"
    );
    Ok(())
}

pub(crate) async fn stage_claude_runtime(
    agent_dir: &Path,
    sandbox: &Sandbox,
) -> miette::Result<()> {
    let (artifact_path, spec) = prepare_artifact(agent_dir).await?;
    activate_artifact(sandbox, &artifact_path, &spec).await
}
#[cfg(test)]
#[path = "claude_runtime_tests.rs"]
mod tests;
