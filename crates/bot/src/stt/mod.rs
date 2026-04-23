//! Speech-to-text pipeline: ffmpeg → PCM → whisper-rs → text.

// Items here are introduced staged across Tasks 8–13 of the voice-STT plan.
// The allow drops naturally once Tasks 10/13 wire them up; if it survives
// past Task 13, that's a sign of a missing wire.
#![allow(dead_code)]

pub mod decode;

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
