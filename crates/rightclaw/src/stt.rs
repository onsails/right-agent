//! Shared STT helpers used by both `rightclaw up` (model download) and
//! `rightclaw-bot` (transcription). Crate root: see `Transcriber` in the bot
//! crate for actual inference.

use std::path::{Path, PathBuf};

use crate::agent::types::WhisperModel;

/// Returns the cache path for a whisper model under the given RIGHTCLAW_HOME.
/// Layout: `<home>/cache/whisper/ggml-<model>.bin`.
pub fn model_cache_path(home: &Path, model: WhisperModel) -> PathBuf {
    home.join("cache").join("whisper").join(model.filename())
}

/// Returns true if `ffmpeg` is on PATH.
pub fn ffmpeg_available() -> bool {
    which::which("ffmpeg").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_cache_path_layout() {
        let home = Path::new("/tmp/.rightclaw");
        let p = model_cache_path(home, WhisperModel::Small);
        assert_eq!(p, Path::new("/tmp/.rightclaw/cache/whisper/ggml-small.bin"));
    }

    #[test]
    fn ffmpeg_available_returns_a_bool() {
        // We don't assert the value — depends on the dev machine. We do
        // assert it doesn't panic and returns a bool.
        let _ = ffmpeg_available();
    }
}
