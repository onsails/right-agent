//! Host-side speech-to-text cache and download helpers.

#![warn(unreachable_pub)]

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use futures::StreamExt;
use right_agent_config::WhisperModel;
use thiserror::Error;
use tokio::io::AsyncWriteExt;

/// Returns the cache path for a whisper model under the given RIGHT_HOME.
/// Layout: `<home>/cache/whisper/ggml-<model>.bin`.
pub fn model_cache_path(home: &Path, model: WhisperModel) -> PathBuf {
    home.join("cache").join("whisper").join(model.filename())
}

/// Returns true if `ffmpeg` is on PATH.
pub fn ffmpeg_available() -> bool {
    which::which("ffmpeg").is_ok()
}

/// Returns true if the final cache file exists. `*.partial` files are ignored.
pub fn is_model_cached(dest: &Path) -> bool {
    dest.exists()
}

/// Error type for [`download_model`].
#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("HTTP status {status} for {url}")]
    BadStatus { status: u16, url: String },
}

/// Test-only helper exercising the same write→flush→atomic-rename sequence
/// `download_model` performs, but on a fixed byte slice instead of a stream.
/// Used by tests to verify the rename invariant without an HTTP fixture.
#[cfg(test)]
async fn write_then_rename(partial: &Path, dest: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = partial.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut f = tokio::fs::File::create(partial).await?;
    f.write_all(bytes).await?;
    f.flush().await?;
    drop(f);
    tokio::fs::rename(partial, dest).await
}

/// Process-wide counter making each partial-download filename unique, so
/// concurrent downloads of the same `dest` never share a partial file.
static PARTIAL_SEQ: AtomicU64 = AtomicU64::new(0);

/// Returns a unique partial-download path for `dest` in the same directory:
/// `<dest-filename>.<pid>.<seq>.partial`. Uniqueness (per-process atomic seq
/// plus pid across processes) is load-bearing: a deterministic `<dest>.partial`
/// let concurrent downloads of one model interleave into a single partial, and
/// the first rename removed it out from under the others, which then failed
/// with `NotFound`. Keeping the partial in `dest`'s directory keeps the final
/// rename atomic (same filesystem).
fn partial_path_for(dest: &Path) -> PathBuf {
    let seq = PARTIAL_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut name = dest
        .file_name()
        .expect("dest must have a filename")
        .to_os_string();
    name.push(format!(".{pid}.{seq}.partial"));
    dest.with_file_name(name)
}

/// Download a whisper model file to `dest`. Streams to `<dest>.partial`
/// (full filename + `.partial` suffix), renames atomically on success. On
/// failure the partial may remain — next call overwrites it.
pub async fn download_model(model: WhisperModel, dest: &Path) -> Result<(), DownloadError> {
    download_url_to_path(model.download_url(), model.filename(), dest).await
}

/// Internal helper: download `url` to `dest`, streaming via a `<dest>.partial`
/// temporary file and atomically renaming on success. `display_name` is used
/// in progress log lines.
async fn download_url_to_path(
    url: &str,
    display_name: &str,
    dest: &Path,
) -> Result<(), DownloadError> {
    ensure_crypto_provider();
    let partial = partial_path_for(dest);

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let resp = reqwest::Client::new().get(url).send().await?;
    if !resp.status().is_success() {
        return Err(DownloadError::BadStatus {
            status: resp.status().as_u16(),
            url: url.to_string(),
        });
    }

    let total = resp.content_length();
    let mut downloaded: u64 = 0;
    let mut last_log_pct: u32 = 0;

    let mut file = tokio::fs::File::create(&partial).await?;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        if let Some(t) = total {
            let pct = ((downloaded * 100) / t) as u32;
            if pct >= last_log_pct + 5 {
                last_log_pct = pct;
                eprintln!(
                    "  {} {pct}% ({:.1}/{:.1} MB)",
                    display_name,
                    downloaded as f64 / (1024.0 * 1024.0),
                    t as f64 / (1024.0 * 1024.0),
                );
            }
        }
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&partial, dest).await?;
    Ok(())
}

fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Public entry: check ffmpeg, then download missing models. Logs warnings
/// (does not error) if download fails — callers should not abort `up`.
pub async fn ensure_models_cached(
    home: &Path,
    models: &HashSet<WhisperModel>,
) -> Result<usize, DownloadError> {
    ensure_models_cached_inner(home, models, ffmpeg_available()).await
}

pub(crate) async fn ensure_models_cached_inner(
    home: &Path,
    models: &HashSet<WhisperModel>,
    ffmpeg_present: bool,
) -> Result<usize, DownloadError> {
    if !ffmpeg_present {
        eprintln!(
            "  ffmpeg not found in PATH — voice transcription disabled. \
             Install: brew install ffmpeg / apt install ffmpeg. \
             Skipping whisper model download."
        );
        return Ok(0);
    }
    let mut downloaded = 0;
    for model in models {
        let dest = model_cache_path(home, *model);
        if is_model_cached(&dest) {
            continue;
        }
        eprintln!(
            "  downloading {} (~{} MB)...",
            model.filename(),
            model.approx_size_mb()
        );
        if let Err(e) = download_model(*model, &dest).await {
            eprintln!("  WARN: download of {} failed: {e}", model.filename());
            continue;
        }
        downloaded += 1;
    }
    Ok(downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn model_cache_path_layout() {
        let home = Path::new("/tmp/.right");
        let p = model_cache_path(home, WhisperModel::Small);
        assert_eq!(p, Path::new("/tmp/.right/cache/whisper/ggml-small.bin"));
    }

    #[test]
    fn ffmpeg_available_returns_a_bool() {
        // We don't assert the value — depends on the dev machine. We do
        // assert it doesn't panic and returns a bool.
        let _ = ffmpeg_available();
    }

    #[tokio::test]
    async fn download_model_writes_to_partial_then_renames() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("ggml-tiny.bin");
        let partial = tmp.path().join("ggml-tiny.bin.partial");

        // Simulate: the download writes 16 bytes to partial then renames.
        write_then_rename(&partial, &dest, b"sixteen-byte-msg")
            .await
            .unwrap();

        assert!(dest.exists(), "final file should exist");
        assert!(!partial.exists(), "partial should be removed after rename");
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), b"sixteen-byte-msg");
    }

    #[test]
    fn partial_path_is_unique_and_suffixed() {
        let dest = Path::new("/tmp/cache/ggml-tiny.bin");
        let a = partial_path_for(dest);
        let b = partial_path_for(dest);

        // Same parent directory as dest (so rename stays on one filesystem).
        assert_eq!(a.parent(), dest.parent());
        // Encodes the dest filename and ends with `.partial`.
        let a_name = a.file_name().unwrap().to_string_lossy();
        assert!(a_name.starts_with("ggml-tiny.bin."), "got {a_name}");
        assert!(a_name.ends_with(".partial"), "got {a_name}");
        // Distinct calls never collide — this is what prevents concurrent
        // downloads of the same model from racing on a shared partial file.
        assert_ne!(a, b);

        // Edge case: dest without an extension still works.
        let no_ext = partial_path_for(Path::new("/tmp/cache/no-ext"));
        assert!(
            no_ext
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("no-ext.")
        );
    }

    /// Regression: three STT tests download the same model into one shared
    /// cache path concurrently (cold CI cache). With a deterministic
    /// `<dest>.partial`, the first rename removed the shared partial and the
    /// others' rename failed with `NotFound`. Unique per-call partials let
    /// every writer rename its own file (last write to `dest` wins).
    #[tokio::test]
    async fn concurrent_downloads_to_same_dest_do_not_race_on_partial() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("cache/whisper/ggml-tiny.bin");

        let mut handles = Vec::new();
        for i in 0..6u8 {
            let dest = dest.clone();
            handles.push(tokio::spawn(async move {
                let partial = partial_path_for(&dest);
                write_then_rename(&partial, &dest, &[i; 64]).await
            }));
        }
        for h in handles {
            h.await
                .unwrap()
                .expect("concurrent write_then_rename must not race on the partial");
        }

        assert!(dest.exists(), "final file should exist after the race");
        // No `.partial` leftovers — every writer renamed its own file away.
        let mut entries = tokio::fs::read_dir(dest.parent().unwrap()).await.unwrap();
        while let Some(e) = entries.next_entry().await.unwrap() {
            let name = e.file_name();
            assert!(
                !name.to_string_lossy().contains(".partial"),
                "leftover partial: {name:?}"
            );
        }
    }

    #[tokio::test]
    async fn download_url_to_path_bad_status_returns_bad_status_error() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("out.bin");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });

        let result = download_url_to_path(&url, "test", &dest).await;
        server.await.unwrap();
        match result {
            Err(DownloadError::BadStatus { status: 404, .. }) => {}
            other => panic!("expected BadStatus(404), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn partial_file_is_ignored_by_cache_check() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("ggml-tiny.bin");
        let partial = tmp.path().join("ggml-tiny.bin.partial");
        tokio::fs::write(&partial, b"junk").await.unwrap();

        assert!(!is_model_cached(&dest), "partial alone is not a cache hit");

        tokio::fs::write(&dest, b"complete").await.unwrap();
        assert!(is_model_cached(&dest), "final file is a cache hit");
    }

    #[tokio::test]
    async fn ensure_models_cached_skips_when_ffmpeg_missing() {
        let tmp = TempDir::new().unwrap();
        let mut models = HashSet::new();
        models.insert(WhisperModel::Small);

        // Simulate ffmpeg-missing by passing the bool explicitly
        let downloaded =
            ensure_models_cached_inner(tmp.path(), &models, /* ffmpeg_present= */ false)
                .await
                .unwrap();

        assert_eq!(downloaded, 0);
        assert!(!model_cache_path(tmp.path(), WhisperModel::Small).exists());
    }

    #[tokio::test]
    async fn ensure_models_cached_skips_already_cached() {
        let tmp = TempDir::new().unwrap();
        let mut models = HashSet::new();
        models.insert(WhisperModel::Small);

        // Pre-populate cache
        let p = model_cache_path(tmp.path(), WhisperModel::Small);
        tokio::fs::create_dir_all(p.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&p, b"already-cached").await.unwrap();

        let downloaded =
            ensure_models_cached_inner(tmp.path(), &models, /* ffmpeg_present= */ true)
                .await
                .unwrap();

        assert_eq!(downloaded, 0);
    }
}
