//! Russian-language markers injected into the user-payload to inform the
//! agent that the original message was voice/video-note.

use crate::stt::SttError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceKind {
    Voice,
    VideoNote,
}

pub fn marker_success(kind: VoiceKind, transcript: &str) -> String {
    match kind {
        VoiceKind::Voice => {
            format!("[Пользователь надиктовал голосовое сообщение. Расшифровка: {transcript:?}]")
        }
        VoiceKind::VideoNote => {
            format!("[Пользователь записал кружок (видео-сообщение). Расшифровка: {transcript:?}]")
        }
    }
}

pub fn marker_for_error(kind: VoiceKind, err: &SttError) -> String {
    let (subject, gendered_too_large) = match kind {
        VoiceKind::Voice => (
            "Пользователь прислал голосовое сообщение",
            "оно слишком большое",
        ),
        VoiceKind::VideoNote => (
            "Пользователь прислал кружок (видео-сообщение)",
            "он слишком большой",
        ),
    };
    match err {
        SttError::FfmpegNotFound => {
            format!("[{subject}, но расшифровка недоступна — на хосте не установлен ffmpeg.]")
        }
        SttError::ModelMissing(_) => format!(
            "[{subject}, но модель распознавания речи не загружена. Запусти 'rightclaw up' заново.]"
        ),
        SttError::FileTooLarge { size_mb, .. } => {
            format!("[{subject}, но {gendered_too_large} для расшифровки ({size_mb} MB).]")
        }
        SttError::FfmpegFailed(_)
        | SttError::WhisperLoadFailed(_)
        | SttError::WhisperInferenceFailed(_) => {
            format!("[{subject}, но расшифровать не удалось: {err}]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn success_voice_includes_transcript_and_kind() {
        let s = marker_success(VoiceKind::Voice, "привет");
        assert!(s.contains("надиктовал голосовое сообщение"));
        assert!(s.contains("\"привет\""));
    }

    #[test]
    fn success_video_note_uses_kruzhok_wording() {
        let s = marker_success(VoiceKind::VideoNote, "hi");
        assert!(s.contains("записал кружок"));
        assert!(s.contains("(видео-сообщение)"));
    }

    #[test]
    fn ffmpeg_missing_voice_marker() {
        let m = marker_for_error(VoiceKind::Voice, &SttError::FfmpegNotFound);
        assert!(m.contains("голосовое сообщение"));
        assert!(m.contains("не установлен ffmpeg"));
    }

    #[test]
    fn ffmpeg_missing_video_note_marker() {
        let m = marker_for_error(VoiceKind::VideoNote, &SttError::FfmpegNotFound);
        assert!(m.contains("кружок"));
        assert!(m.contains("не установлен ffmpeg"));
    }

    #[test]
    fn model_missing_marker() {
        let m = marker_for_error(
            VoiceKind::Voice,
            &SttError::ModelMissing(PathBuf::from("/x")),
        );
        assert!(m.contains("rightclaw up"));
    }

    #[test]
    fn file_too_large_uses_gendered_form() {
        let voice = marker_for_error(
            VoiceKind::Voice,
            &SttError::FileTooLarge {
                size_mb: 30,
                max_mb: 25,
            },
        );
        assert!(voice.contains("оно слишком большое"));
        assert!(voice.contains("30 MB"));

        let circle = marker_for_error(
            VoiceKind::VideoNote,
            &SttError::FileTooLarge {
                size_mb: 30,
                max_mb: 25,
            },
        );
        assert!(circle.contains("он слишком большой"));
    }

    #[test]
    fn generic_failure_includes_short_reason() {
        let m = marker_for_error(
            VoiceKind::Voice,
            &SttError::WhisperInferenceFailed("oom".into()),
        );
        assert!(m.contains("расшифровать не удалось"));
        assert!(m.contains("oom"));
    }
}
