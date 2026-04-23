//! Speech-to-text pipeline: ffmpeg → PCM → whisper-rs → text.

// Items here are introduced staged across Tasks 8–13 of the voice-STT plan.
// The allow drops naturally once Tasks 10/13 wire them up; if it survives
// past Task 13, that's a sign of a missing wire.
#![allow(dead_code)]

pub mod decode;
pub mod markers;
pub mod whisper;

use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SttError {
    #[error("ffmpeg not found in PATH")]
    FfmpegNotFound,
    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(String),
    #[error("whisper model file missing: {0}")]
    ModelMissing(PathBuf),
    #[error("failed to load whisper model: {0}")]
    WhisperLoadFailed(String),
    #[error("whisper inference failed: {0}")]
    WhisperInferenceFailed(String),
    #[error("audio file too large: {size_mb} MB (max {max_mb} MB)")]
    FileTooLarge { size_mb: u64, max_mb: u64 },
}

pub const MAX_AUDIO_FILE_MB: u64 = 25;

#[derive(Debug)]
pub struct TranscriptionResult {
    pub text: String,
    pub detected_language: Option<String>,
}

pub struct Transcriber {
    engine: whisper::WhisperEngine,
}

impl Transcriber {
    pub fn new(model_path: std::path::PathBuf) -> Self {
        Self {
            engine: whisper::WhisperEngine::new(model_path),
        }
    }

    pub async fn transcribe_voice(
        &self,
        file: &std::path::Path,
    ) -> Result<TranscriptionResult, SttError> {
        self.transcribe_inner(file).await
    }

    pub async fn transcribe_video_note(
        &self,
        file: &std::path::Path,
    ) -> Result<TranscriptionResult, SttError> {
        self.transcribe_inner(file).await
    }

    async fn transcribe_inner(
        &self,
        file: &std::path::Path,
    ) -> Result<TranscriptionResult, SttError> {
        let meta = tokio::fs::metadata(file)
            .await
            .map_err(|e| SttError::FfmpegFailed(format!("stat {}: {e}", file.display())))?;
        let size_mb = meta.len() / (1024 * 1024);
        if size_mb > MAX_AUDIO_FILE_MB {
            return Err(SttError::FileTooLarge {
                size_mb,
                max_mb: MAX_AUDIO_FILE_MB,
            });
        }
        let samples = decode::decode_to_pcm_f32(file).await?;
        let (text, lang) = self.engine.transcribe(samples).await?;
        Ok(TranscriptionResult {
            text,
            detected_language: lang,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rightclaw::agent::types::WhisperModel;
    use rightclaw::stt::model_cache_path;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    async fn tiny_path() -> PathBuf {
        let home = std::env::var_os("RIGHTCLAW_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".rightclaw"));
        let p = model_cache_path(&home, WhisperModel::Tiny);
        if !p.exists() {
            rightclaw::stt::download_model(WhisperModel::Tiny, &p)
                .await
                .unwrap();
        }
        p
    }

    #[tokio::test]
    async fn transcribe_voice_end_to_end() {
        let t = Transcriber::new(tiny_path().await);
        let res = t.transcribe_voice(&fixture("hello.oga")).await.unwrap();
        assert!(res.text.to_lowercase().contains("test"));
    }

    #[tokio::test]
    async fn transcribe_video_note_end_to_end() {
        let t = Transcriber::new(tiny_path().await);
        let res = t
            .transcribe_video_note(&fixture("circle.mp4"))
            .await
            .unwrap();
        assert!(res.text.to_lowercase().contains("test"));
    }

    #[tokio::test]
    async fn file_too_large_returns_error() {
        // Create a 30MB sparse file — no actual disk space used on most filesystems.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let f = tokio::fs::File::create(tmp.path()).await.unwrap();
        f.set_len(30 * 1024 * 1024).await.unwrap();
        drop(f);
        let t = Transcriber::new(PathBuf::from("/nonexistent.bin"));
        match t.transcribe_voice(tmp.path()).await {
            Err(SttError::FileTooLarge {
                size_mb: 30,
                max_mb: 25,
            }) => {}
            other => panic!("expected FileTooLarge, got {other:?}"),
        }
    }
}
