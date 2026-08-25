//! Host-side acquisition and guest activation of the pinned Claude Code runtime.

use std::fs::{OpenOptions, Permissions};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::{FileExt, TryLockError};
use futures::StreamExt as _;
use right_sandbox::ExecRequest;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

use crate::sandbox::Sandbox;

const CLAUDE_VERSION: &str = "2.1.241";
const ARTIFACT_BASE_URL: &str = "https://downloads.claude.ai/claude-code-releases";
const CLAUDE_ROOT: &str = "/opt/right";
const CLAUDE_RUNTIME_DIR: &str = "/opt/right/claude";
const CLAUDE_BIN_DIR: &str = "/opt/right/bin";
pub(crate) const CLAUDE_ACTIVE_LINK: &str = "/opt/right/bin/claude";
const GUEST_OPERATION_TIMEOUT: Duration = Duration::from_secs(120);
const GUEST_UPLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const CACHE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const CACHE_LOCK_RETRY: Duration = Duration::from_millis(50);
const CACHE_DIR_MODE: u32 = 0o700;
const ARTIFACT_MODE: u32 = 0o555;
#[cfg(target_os = "linux")]
const O_NOFOLLOW_FLAG: i32 = 0x20_000;
#[cfg(target_os = "macos")]
const O_NOFOLLOW_FLAG: i32 = 0x100;

#[derive(Debug)]
pub(crate) enum ClaudeRuntimeError {
    Hard(miette::Report),
    Retryable(miette::Report),
}

impl std::fmt::Display for ClaudeRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hard(error) => {
                write!(formatter, "invalid Claude runtime configuration: {error:#}")
            }
            Self::Retryable(error) => write!(
                formatter,
                "temporary Claude runtime staging failure: {error:#}"
            ),
        }
    }
}

impl std::error::Error for ClaudeRuntimeError {}

impl miette::Diagnostic for ClaudeRuntimeError {}

impl ClaudeRuntimeError {
    pub(crate) fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}

type RuntimeResult<T> = Result<T, ClaudeRuntimeError>;

fn hard(error: miette::Report) -> ClaudeRuntimeError {
    ClaudeRuntimeError::Hard(error)
}

fn retryable(error: miette::Report) -> ClaudeRuntimeError {
    ClaudeRuntimeError::Retryable(error)
}

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
        format!("{CLAUDE_RUNTIME_DIR}/{}", self.cache_name())
    }
}

#[derive(Debug, Clone)]
struct ArtifactSource {
    base_url: String,
    cache_root: PathBuf,
    spec: ArtifactSpec,
}

/// microsandbox resolves images with `Platform::host_linux()`: the guest CPU
/// architecture is therefore the host CPU architecture, with a Linux ABI.
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
        unsupported => miette::bail!(
            "unsupported host/Claude guest architecture {unsupported}; microsandbox Platform::host_linux requires arm64 or x86_64"
        ),
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

async fn prepare_artifact(agent_dir: &Path) -> RuntimeResult<(PathBuf, ArtifactSpec)> {
    let cache_root = cache_root_for_agent(agent_dir).map_err(hard)?;
    let spec = artifact_spec_for_arch(std::env::consts::ARCH).map_err(hard)?;
    let source = ArtifactSource {
        base_url: ARTIFACT_BASE_URL.to_owned(),
        cache_root,
        spec,
    };
    let path = prepare_artifact_from_typed(&source).await?;
    Ok((path, source.spec))
}

async fn prepare_artifact_from_typed(source: &ArtifactSource) -> RuntimeResult<PathBuf> {
    ensure_secure_cache_root(&source.cache_root)
        .await
        .map_err(hard)?;
    let lock_path = source.cache_root.join(".download.lock");
    if let Ok(metadata) = tokio::fs::symlink_metadata(&lock_path).await
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(hard(miette::miette!(
            "Claude cache lock is not a regular file: {}",
            lock_path.display()
        )));
    }
    let _lock = acquire_cache_lock(&lock_path).await.map_err(retryable)?;
    prepare_artifact_locked(source).await
}

async fn ensure_secure_cache_root(path: &Path) -> miette::Result<()> {
    for component in path.ancestors().skip(1).take(3) {
        let metadata = std::fs::symlink_metadata(component).map_err(|e| {
            miette::miette!(
                "inspect Claude cache ancestor {}: {e:#}",
                component.display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            miette::bail!(
                "Claude cache ancestor is not a regular directory: {}",
                component.display()
            );
        }
    }
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                miette::bail!(
                    "Claude cache root is not a regular directory: {}",
                    path.display()
                );
            }
            let parent_uid = path
                .parent()
                .and_then(|parent| std::fs::metadata(parent).ok())
                .map(|metadata| metadata.uid())
                .ok_or_else(|| {
                    miette::miette!("inspect Claude cache parent for {}", path.display())
                })?;
            if metadata.uid() != parent_uid {
                miette::bail!(
                    "Claude cache root owner differs from its parent: {}",
                    path.display()
                );
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir_all(path).await.map_err(|e| {
                miette::miette!("create Claude artifact cache {}: {e:#}", path.display())
            })?;
        }
        Err(error) => {
            return Err(miette::miette!(
                "inspect Claude cache root {}: {error:#}",
                path.display()
            ));
        }
    }
    tokio::fs::set_permissions(path, Permissions::from_mode(CACHE_DIR_MODE))
        .await
        .map_err(|e| miette::miette!("secure Claude cache root {}: {e:#}", path.display()))?;
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|e| miette::miette!("reinspect Claude cache root {}: {e:#}", path.display()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.mode() & 0o777 != CACHE_DIR_MODE
    {
        miette::bail!(
            "Claude cache root failed secure-directory validation: {}",
            path.display()
        );
    }
    Ok(())
}

struct CacheLock {
    path: PathBuf,
    file: std::fs::File,
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(path = %self.path.display(), %error, "failed to release Claude cache lock");
        }
    }
}

async fn acquire_cache_lock(path: &Path) -> miette::Result<CacheLock> {
    if let Ok(metadata) = tokio::fs::symlink_metadata(path).await
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        miette::bail!(
            "Claude cache lock is not a regular file: {}",
            path.display()
        );
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW_FLAG)
        .open(path)
        .map_err(|e| miette::miette!("open Claude cache lock {}: {e:#}", path.display()))?;
    let path_metadata = std::fs::symlink_metadata(path)
        .map_err(|e| miette::miette!("reinspect Claude cache lock {}: {e:#}", path.display()))?;
    let metadata = file.metadata().map_err(|e| {
        miette::miette!("inspect opened Claude cache lock {}: {e:#}", path.display())
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() || !metadata.is_file() {
        miette::bail!(
            "Claude cache lock is not a stable regular file: {}",
            path.display()
        );
    }
    if path_metadata.dev() != metadata.dev() || path_metadata.ino() != metadata.ino() {
        miette::bail!(
            "Claude cache lock changed while opening: {}",
            path.display()
        );
    }
    let started = tokio::time::Instant::now();
    loop {
        match FileExt::try_lock(&file) {
            Ok(()) => {
                return Ok(CacheLock {
                    path: path.to_owned(),
                    file,
                });
            }
            Err(TryLockError::WouldBlock) if started.elapsed() < CACHE_LOCK_TIMEOUT => {
                tokio::time::sleep(std::cmp::min(
                    CACHE_LOCK_RETRY,
                    CACHE_LOCK_TIMEOUT.saturating_sub(started.elapsed()),
                ))
                .await;
            }
            Err(TryLockError::WouldBlock) => {
                miette::bail!("timed out waiting for Claude cache lock {}", path.display())
            }
            Err(TryLockError::Error(error)) => {
                return Err(miette::miette!(
                    "acquire Claude cache lock {}: {error:#}",
                    path.display()
                ));
            }
        }
    }
}

#[cfg(test)]
async fn prepare_artifact_from(source: &ArtifactSource) -> miette::Result<PathBuf> {
    prepare_artifact_from_typed(source)
        .await
        .map_err(miette::Report::new)
}

async fn prune_stale_downloads(cache_root: &Path) -> miette::Result<()> {
    let mut entries = tokio::fs::read_dir(cache_root)
        .await
        .map_err(|e| miette::miette!("list Claude cache {}: {e:#}", cache_root.display()))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| miette::miette!("read Claude cache entry: {e:#}"))?
    {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".claude-")
            && name.to_string_lossy().ends_with(".download")
        {
            let metadata = tokio::fs::symlink_metadata(entry.path())
                .await
                .map_err(|e| miette::miette!("inspect stale Claude download: {e:#}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                miette::bail!(
                    "unexpected stale Claude cache entry: {}",
                    entry.path().display()
                );
            }
            tokio::fs::remove_file(entry.path())
                .await
                .map_err(|e| miette::miette!("remove stale Claude download: {e:#}"))?;
        }
    }
    Ok(())
}

async fn prepare_artifact_locked(source: &ArtifactSource) -> RuntimeResult<PathBuf> {
    prune_stale_downloads(&source.cache_root)
        .await
        .map_err(hard)?;
    let final_path = source.cache_root.join(source.spec.cache_name());
    if let Ok(metadata) = tokio::fs::symlink_metadata(&final_path).await {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(hard(miette::miette!(
                "Claude cache entry is not a regular file: {}",
                final_path.display()
            )));
        }
        if artifact_matches(&final_path, &source.spec)
            .await
            .map_err(retryable)?
        {
            tokio::fs::set_permissions(&final_path, Permissions::from_mode(ARTIFACT_MODE))
                .await
                .map_err(|e| {
                    retryable(miette::miette!(
                        "reassert cached Claude artifact mode: {e:#}"
                    ))
                })?;
            return Ok(final_path);
        }
        tokio::fs::remove_file(&final_path).await.map_err(|e| {
            hard(miette::miette!(
                "remove corrupt Claude cache entry {}: {e:#}",
                final_path.display()
            ))
        })?;
    }

    let temp_path = source.cache_root.join(format!(
        ".{}.{}.download",
        source.spec.cache_name(),
        uuid::Uuid::new_v4()
    ));
    let download = download_verified(source, &temp_path).await;
    if let Err(error) = download {
        if let Err(cleanup_error) = tokio::fs::remove_file(&temp_path).await
            && cleanup_error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %temp_path.display(), %cleanup_error, "failed to remove incomplete Claude download");
        }
        return Err(error);
    }
    tokio::fs::set_permissions(&temp_path, Permissions::from_mode(ARTIFACT_MODE))
        .await
        .map_err(|e| retryable(miette::miette!("set verified Claude artifact mode: {e:#}")))?;
    if let Err(error) = tokio::fs::rename(&temp_path, &final_path).await {
        if let Err(cleanup_error) = tokio::fs::remove_file(&temp_path).await
            && cleanup_error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %temp_path.display(), %cleanup_error, "failed to remove unpublished Claude download");
        }
        return Err(retryable(miette::miette!(
            "publish verified Claude artifact {}: {error:#}",
            final_path.display()
        )));
    }
    Ok(final_path)
}

async fn artifact_matches(path: &Path, spec: &ArtifactSpec) -> miette::Result<bool> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(miette::miette!(
                "stat cached Claude artifact {}: {error:#}",
                path.display()
            ));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != spec.size {
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

fn http_client() -> miette::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_TOTAL_TIMEOUT)
        .build()
        .map_err(|e| miette::miette!("build Claude artifact HTTP client: {e:#}"))
}

async fn download_verified(source: &ArtifactSource, destination: &Path) -> RuntimeResult<()> {
    let url = format!(
        "{}/{}/{}/claude",
        source.base_url.trim_end_matches('/'),
        source.spec.version,
        source.spec.platform
    );
    let response = http_client()
        .map_err(hard)?
        .get(&url)
        .send()
        .await
        .map_err(|e| {
            retryable(miette::miette!(
                "download pinned Claude artifact {url}: {e:#}"
            ))
        })?
        .error_for_status()
        .map_err(|e| {
            retryable(miette::miette!(
                "download pinned Claude artifact {url}: {e:#}"
            ))
        })?;
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(destination)
        .await
        .map_err(|e| {
            retryable(miette::miette!(
                "create Claude artifact download {}: {e:#}",
                destination.display()
            ))
        })?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| retryable(miette::miette!("stream Claude artifact {url}: {e:#}")))?;
        size += chunk.len() as u64;
        if size > source.spec.size {
            return Err(hard(miette::miette!(
                "Claude artifact size mismatch: expected {}, received more than {} bytes",
                source.spec.size,
                source.spec.size
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| retryable(miette::miette!("write Claude artifact download: {e:#}")))?;
    }
    file.flush()
        .await
        .map_err(|e| retryable(miette::miette!("flush Claude artifact download: {e:#}")))?;
    let digest = digest_hex(&hasher.finalize());
    if size != source.spec.size {
        return Err(hard(miette::miette!(
            "Claude artifact size mismatch: expected {}, received {size} bytes",
            source.spec.size
        )));
    }
    if digest != source.spec.sha256 {
        return Err(hard(miette::miette!(
            "Claude artifact SHA-256 mismatch: expected {}, received {digest}",
            source.spec.sha256
        )));
    }
    file.sync_all().await.map_err(|e| {
        retryable(miette::miette!(
            "sync verified Claude artifact download: {e:#}"
        ))
    })?;
    Ok(())
}

#[derive(Debug)]
struct GuestActivationPlan {
    target: String,
    upload: String,
    temporary_link: String,
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
    let upload = format!("{CLAUDE_RUNTIME_DIR}/.upload-{nonce}");
    let temporary_link = format!("{CLAUDE_BIN_DIR}/.claude-link-{nonce}");
    GuestActivationPlan {
        chmod: root_request("chmod", &["0555", &upload]),
        hash: root_request("sha256sum", &[&upload]),
        publish: root_request("mv", &["-fT", &upload, &target]),
        create_link: root_request("ln", &["-s", &target, &temporary_link]),
        activate_link: root_request("mv", &["-fT", &temporary_link, CLAUDE_ACTIVE_LINK]),
        target,
        upload,
        temporary_link,
    }
}

async fn run_root_request(sandbox: &Sandbox, request: &ExecRequest) -> miette::Result<String> {
    let outcome = sandbox.exec(request).await.map_err(|e| {
        miette::miette!(
            "run root-owned guest command {} {:?}: {e:#}",
            request.cmd,
            request.args
        )
    })?;
    if outcome.code != 0 {
        miette::bail!(
            "root-owned guest command {} {:?} exited {}: {}{}",
            request.cmd,
            request.args,
            outcome.code,
            String::from_utf8_lossy(&outcome.stdout),
            String::from_utf8_lossy(&outcome.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&outcome.stdout).into_owned())
}
async fn ensure_root_owned_dirs(sandbox: &Sandbox) -> RuntimeResult<()> {
    run_root_request(
        sandbox,
        &root_request("mkdir", &["-p", CLAUDE_RUNTIME_DIR, CLAUDE_BIN_DIR]),
    )
    .await
    .map_err(retryable)?;
    for path in ["/opt", CLAUDE_ROOT, CLAUDE_RUNTIME_DIR, CLAUDE_BIN_DIR] {
        let metadata =
            run_root_request(sandbox, &root_request("stat", &["-c", "%F:%u:%g:%a", path]))
                .await
                .map_err(retryable)?;
        let fields = metadata.trim().split(':').collect::<Vec<_>>();
        let mode = fields
            .get(3)
            .and_then(|mode| u32::from_str_radix(mode, 8).ok());
        if fields.first() != Some(&"directory")
            || fields.get(1) != Some(&"0")
            || fields.get(2) != Some(&"0")
            || mode.is_none_or(|mode| mode & 0o022 != 0)
        {
            return Err(hard(miette::miette!(
                "refusing unsafe Claude runtime directory {path}: {}",
                metadata.trim()
            )));
        }
        let no_follow = sandbox
            .exec(&root_request("test", &["!", "-L", path]))
            .await
            .map_err(|e| {
                retryable(miette::miette!(
                    "validate no-follow Claude directory {path}: {e:#}"
                ))
            })?;
        if no_follow.code != 0 {
            return Err(hard(miette::miette!(
                "refusing symlink Claude runtime directory {path}"
            )));
        }
    }
    Ok(())
}

async fn cleanup_guest_path(sandbox: &Sandbox, path: &str) {
    if let Err(error) = run_root_request(sandbox, &root_request("rm", &["-rf", path])).await {
        tracing::warn!(%path, error = %format!("{error:#}"), "failed to clean stale Claude staging path");
    }
}

async fn prune_guest_staging(sandbox: &Sandbox) -> miette::Result<()> {
    run_root_request(
        sandbox,
        &root_request(
            "find",
            &[
                CLAUDE_RUNTIME_DIR,
                "-maxdepth",
                "1",
                "-name",
                ".upload-*",
                "-delete",
            ],
        ),
    )
    .await?;
    run_root_request(
        sandbox,
        &root_request(
            "find",
            &[
                CLAUDE_BIN_DIR,
                "-maxdepth",
                "1",
                "-name",
                ".claude-link-*",
                "-delete",
            ],
        ),
    )
    .await?;
    Ok(())
}

async fn guest_artifact_matches(
    sandbox: &Sandbox,
    path: &str,
    spec: &ArtifactSpec,
) -> miette::Result<bool> {
    let test = sandbox
        .exec(&root_request("test", &["-f", path]))
        .await
        .map_err(|e| miette::miette!("test guest Claude artifact {path}: {e:#}"))?;
    if test.code != 0 {
        return Ok(false);
    }
    let metadata =
        run_root_request(sandbox, &root_request("stat", &["-c", "%u:%a:%s", path])).await?;
    let expected = format!("0:555:{}", spec.size);
    if metadata.trim() != expected {
        return Ok(false);
    }
    let output = run_root_request(sandbox, &root_request("sha256sum", &[path])).await?;
    Ok(output.split_whitespace().next() == Some(spec.sha256.as_str()))
}

async fn gc_guest_versions(sandbox: &Sandbox, active: &str) -> miette::Result<()> {
    let output = run_root_request(
        sandbox,
        &root_request(
            "find",
            &[
                CLAUDE_RUNTIME_DIR,
                "-maxdepth",
                "1",
                "-type",
                "f",
                "-name",
                "claude-*",
                "-printf",
                "%T@ %p\n",
            ],
        ),
    )
    .await?;
    let mut entries = output
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter_map(|(mtime, path)| mtime.parse::<f64>().ok().map(|mtime| (mtime, path)))
        .filter(|(_, path)| *path != active)
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.0.total_cmp(&left.0));
    for (_, stale) in entries.into_iter().skip(1) {
        run_root_request(sandbox, &root_request("rm", &["-f", stale])).await?;
    }
    Ok(())
}

async fn activate_artifact(
    sandbox: &Sandbox,
    artifact_path: &Path,
    spec: &ArtifactSpec,
) -> RuntimeResult<()> {
    ensure_root_owned_dirs(sandbox).await?;
    prune_guest_staging(sandbox).await.map_err(retryable)?;
    // `test -L` observes the link itself. The SDK fs_stat follows links and must
    // not be used for this repeat-activation contract.
    let active = sandbox
        .exec(&root_request("test", &["-L", CLAUDE_ACTIVE_LINK]))
        .await
        .map_err(|e| retryable(miette::miette!("validate active Claude link: {e:#}")))?;
    let active_exists = sandbox
        .exec(&root_request("test", &["-e", CLAUDE_ACTIVE_LINK]))
        .await
        .map_err(|e| retryable(miette::miette!("check active Claude path: {e:#}")))?;
    if active.code != 0 && active_exists.code == 0 {
        return Err(hard(miette::miette!(
            "refusing non-symlink active Claude path {CLAUDE_ACTIVE_LINK}"
        )));
    }

    let plan = guest_activation_plan(spec, &uuid::Uuid::new_v4().simple().to_string());
    if !guest_artifact_matches(sandbox, &plan.target, spec)
        .await
        .map_err(retryable)?
    {
        cleanup_guest_path(sandbox, &plan.target).await;
        let upload = tokio::time::timeout(
            GUEST_UPLOAD_TIMEOUT,
            sandbox.fs_copy_from_host(artifact_path, &plan.upload),
        )
        .await;
        match upload {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                cleanup_guest_path(sandbox, &plan.upload).await;
                return Err(retryable(miette::miette!(
                    "upload verified Claude artifact {} -> {}: {error:#}",
                    artifact_path.display(),
                    plan.upload
                )));
            }
            Err(_) => {
                cleanup_guest_path(sandbox, &plan.upload).await;
                return Err(retryable(miette::miette!(
                    "upload verified Claude artifact timed out after {}s",
                    GUEST_UPLOAD_TIMEOUT.as_secs()
                )));
            }
        }
        let verification = async {
            run_root_request(sandbox, &plan.chmod).await?;
            let hash_output = run_root_request(sandbox, &plan.hash).await?;
            if hash_output.split_whitespace().next() != Some(spec.sha256.as_str()) {
                miette::bail!("guest Claude artifact SHA-256 mismatch after upload");
            }
            let metadata = run_root_request(
                sandbox,
                &root_request("stat", &["-c", "%u:%a:%s", &plan.upload]),
            )
            .await?;
            if metadata.trim() != format!("0:555:{}", spec.size) {
                miette::bail!(
                    "guest Claude artifact metadata mismatch after upload: {}",
                    metadata.trim()
                );
            }
            run_root_request(sandbox, &plan.publish).await?;
            Ok::<(), miette::Report>(())
        }
        .await;
        if let Err(error) = verification {
            cleanup_guest_path(sandbox, &plan.upload).await;
            return Err(hard(error));
        }
    } else {
        run_root_request(sandbox, &root_request("chmod", &["0555", &plan.target]))
            .await
            .map_err(retryable)?;
        if !guest_artifact_matches(sandbox, &plan.target, spec)
            .await
            .map_err(retryable)?
        {
            return Err(hard(miette::miette!(
                "cached guest Claude artifact is writable or failed verification after mode reassertion"
            )));
        }
    }

    if let Err(error) = run_root_request(sandbox, &plan.create_link).await {
        cleanup_guest_path(sandbox, &plan.temporary_link).await;
        return Err(retryable(error));
    }
    if let Err(error) = run_root_request(sandbox, &plan.activate_link).await {
        cleanup_guest_path(sandbox, &plan.temporary_link).await;
        return Err(retryable(error));
    }
    if let Err(error) = gc_guest_versions(sandbox, &plan.target).await {
        tracing::warn!(error = %format!("{error:#}"), "failed to garbage-collect old Claude runtimes after activation");
    }
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
) -> Result<(), ClaudeRuntimeError> {
    let (artifact_path, spec) = prepare_artifact(agent_dir).await?;
    if !artifact_matches(&artifact_path, &spec)
        .await
        .map_err(retryable)?
    {
        return Err(hard(miette::miette!(
            "verified host Claude cache entry changed before guest upload"
        )));
    }
    activate_artifact(sandbox, &artifact_path, &spec).await
}

#[cfg(test)]
#[path = "claude_runtime_tests.rs"]
mod tests;
