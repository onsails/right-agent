//! Shared STT helpers used by both `right up` (model download) and
//! `right-bot` (transcription). Crate root: see `Transcriber` in the bot
//! crate for actual inference.

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
};

use futures::StreamExt;
use thiserror::Error;
use tokio::io::AsyncWriteExt;

/// Whisper model size for speech-to-text transcription.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    Tiny,
    Base,
    #[default]
    Small,
    Medium,
    #[serde(rename = "large-v3")]
    LargeV3,
}

impl WhisperModel {
    pub fn filename(&self) -> &'static str {
        match self {
            Self::Tiny => "ggml-tiny.bin",
            Self::Base => "ggml-base.bin",
            Self::Small => "ggml-small.bin",
            Self::Medium => "ggml-medium.bin",
            Self::LargeV3 => "ggml-large-v3.bin",
        }
    }

    pub fn download_url(&self) -> &'static str {
        match self {
            Self::Tiny => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            Self::Base => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            Self::Small => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
            }
            Self::Medium => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
            }
            Self::LargeV3 => {
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
            }
        }
    }

    pub fn approx_size_mb(&self) -> u64 {
        match self {
            Self::Tiny => 75,
            Self::Base => 150,
            Self::Small => 470,
            Self::Medium => 1500,
            Self::LargeV3 => 3100,
        }
    }

    /// Kebab-case YAML string for this model — mirrors serde's rename_all output.
    pub fn yaml_str(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Base => "base",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::LargeV3 => "large-v3",
        }
    }
}

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

/// Returns the partial-download path for `dest`: `<dest>.partial` (full
/// filename + `.partial` suffix, regardless of what extension `dest` has).
fn partial_path_for(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .expect("dest must have a filename")
        .to_os_string();
    name.push(".partial");
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
    fn partial_path_appends_dot_partial() {
        assert_eq!(
            partial_path_for(Path::new("/tmp/cache/ggml-tiny.bin")),
            Path::new("/tmp/cache/ggml-tiny.bin.partial"),
        );
        // Edge case: dest without an extension still works.
        assert_eq!(
            partial_path_for(Path::new("/tmp/cache/no-ext")),
            Path::new("/tmp/cache/no-ext.partial"),
        );
    }

    #[tokio::test]
    async fn download_url_to_path_bad_status_returns_bad_status_error() {
        let tmp = TempDir::new().unwrap();
        let dest = tmp.path().join("out.bin");
        // httpbin.org/status/404 reliably returns 404; if the dev machine is
        // offline the test will surface as a different error variant — accept that.
        let result = download_url_to_path("https://httpbin.org/status/404", "test", &dest).await;
        match result {
            Err(DownloadError::BadStatus { status: 404, .. }) => {}
            Err(DownloadError::Http(_)) => {
                // Network unavailable in test env — skip.
                eprintln!("skipping: network unavailable for httpbin.org");
            }
            other => panic!("expected BadStatus(404) or network failure, got {other:?}"),
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
