//! Wrap a system `ffmpeg` subprocess that converts an arbitrary input file to
//! PCM f32 little-endian @ 16 kHz mono, returned as `Vec<f32>`.

use crate::stt::SttError;
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub async fn decode_to_pcm_f32(input: &Path) -> Result<Vec<f32>, SttError> {
    if which::which("ffmpeg").is_err() {
        return Err(SttError::FfmpegNotFound);
    }

    let mut child = Command::new("ffmpeg")
        .arg("-nostdin")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(input)
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg("-f")
        .arg("f32le")
        .arg("pipe:1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SttError::FfmpegFailed(format!("spawn: {e}")))?;

    // Drain stdout and stderr concurrently — sequential reads risk a
    // deadlock if ffmpeg fills the stderr pipe buffer (~64 KB) while we
    // are still reading stdout. `-loglevel error` keeps stderr small in
    // the happy path, but a corrupt input can trigger verbose probe noise.
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr_reader = child.stderr.take().expect("piped");
    let mut bytes = Vec::new();
    let mut err = String::new();
    let (stdout_res, _) = tokio::join!(
        stdout.read_to_end(&mut bytes),
        stderr_reader.read_to_string(&mut err),
    );
    stdout_res.map_err(|e| SttError::FfmpegFailed(format!("read stdout: {e}")))?;

    let status = child
        .wait()
        .await
        .map_err(|e| SttError::FfmpegFailed(format!("wait: {e}")))?;
    if !status.success() {
        let trimmed: String = err.chars().take(200).collect();
        return Err(SttError::FfmpegFailed(trimmed));
    }

    // f32 little-endian; 4 bytes per sample.
    if bytes.len() % 4 != 0 {
        return Err(SttError::FfmpegFailed(format!(
            "unaligned PCM stream: {} bytes",
            bytes.len()
        )));
    }
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    // hello.oga is ~0.85s → ~13.6k samples at 16 kHz.
    // We use 5_000 as the threshold (>0.3s) to catch "no samples" bugs
    // while working with the actual fixture duration.
    #[tokio::test]
    async fn decode_voice_oga_yields_samples() {
        let samples = decode_to_pcm_f32(&fixture("hello.oga"))
            .await
            .expect("ffmpeg required (install: brew install ffmpeg / apt install ffmpeg)");
        assert!(
            samples.len() > 5_000,
            "got {} samples, want >5_000 (~0.3s @16k)",
            samples.len()
        );
    }

    #[tokio::test]
    async fn decode_video_note_mp4_yields_samples() {
        let samples = decode_to_pcm_f32(&fixture("circle.mp4"))
            .await
            .expect("ffmpeg required");
        assert!(
            samples.len() > 5_000,
            "got {} samples, want >5_000",
            samples.len()
        );
    }

    #[tokio::test]
    async fn decode_corrupted_input_returns_error() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(tmp.path(), b"not audio").await.unwrap();
        match decode_to_pcm_f32(tmp.path()).await {
            Err(SttError::FfmpegFailed(_)) => {}
            other => panic!("expected FfmpegFailed, got {other:?}"),
        }
    }
}
