//! Lazy holder for a `WhisperContext`. One context per `WhisperEngine`,
//! shared via `Arc<Mutex<...>>` so concurrent inferences serialize.

use crate::stt::SttError;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::task;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub(crate) struct WhisperEngine {
    model_path: PathBuf,
    ctx: OnceLock<Arc<Mutex<WhisperContext>>>,
}

impl WhisperEngine {
    pub(crate) fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
            ctx: OnceLock::new(),
        }
    }

    fn ensure_ctx(&self) -> Result<Arc<Mutex<WhisperContext>>, SttError> {
        if let Some(c) = self.ctx.get() {
            return Ok(c.clone());
        }
        if !self.model_path.exists() {
            return Err(SttError::ModelMissing(self.model_path.clone()));
        }
        let ctx =
            WhisperContext::new_with_params(&self.model_path, WhisperContextParameters::default())
                .map_err(|e| SttError::WhisperLoadFailed(format!("{e}")))?;
        let arc = Arc::new(Mutex::new(ctx));
        // If another thread raced us here, drop our copy and use the
        // winner's. Without this, both Arcs stay alive while the loser's
        // transcribe() call holds it — ~470MB orphaned per race.
        match self.ctx.set(arc) {
            Ok(()) => Ok(self.ctx.get().expect("just set").clone()),
            Err(_orphan) => Ok(self.ctx.get().expect("loser of race; winner set first").clone()),
        }
    }

    /// Run whisper on PCM f32 16 kHz mono. Returns the transcript and the
    /// detected language (if any).
    pub(crate) async fn transcribe(
        &self,
        samples: Vec<f32>,
    ) -> Result<(String, Option<String>), SttError> {
        let ctx = self.ensure_ctx()?;
        task::spawn_blocking(move || {
            let ctx = ctx.lock().expect("whisper mutex poisoned");
            let mut state = ctx
                .create_state()
                .map_err(|e| SttError::WhisperInferenceFailed(format!("{e}")))?;

            // Build params inside the closure — FullParams has lifetime params
            // and must not cross the spawn_blocking boundary with borrowed data.
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_print_special(false);
            // language=None → autodetect (clears the default "en" pointer)
            params.set_language(None);

            state
                .full(params, &samples)
                .map_err(|e| SttError::WhisperInferenceFailed(format!("{e}")))?;

            let n = state.full_n_segments();
            let mut text = String::new();
            for i in 0..n {
                if let Some(seg) = state.get_segment(i) {
                    // to_str_lossy avoids hard-failing on rare invalid-UTF-8
                    // tokens around multilingual transitions; bad bytes
                    // become U+FFFD instead of aborting the whole transcript.
                    let s = seg
                        .to_str_lossy()
                        .map_err(|e| SttError::WhisperInferenceFailed(format!("{e}")))?;
                    text.push_str(&s);
                }
            }

            // full_lang_id_from_state() returns -1 when unknown/not detected.
            let lang = {
                let id = state.full_lang_id_from_state();
                if id >= 0 {
                    whisper_rs::get_lang_str(id).map(|s| s.to_string())
                } else {
                    None
                }
            };

            Ok((text.trim().to_string(), lang))
        })
        .await
        .map_err(|e| SttError::WhisperInferenceFailed(format!("join: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stt::decode::decode_to_pcm_f32;
    use right_core::stt::{download_model, model_cache_path, WhisperModel};
    use std::path::PathBuf;

    /// Returns a path to a cached ggml-tiny.bin under the user's
    /// RIGHT_HOME or `~/.right`. Downloads if missing.
    async fn ensure_tiny_model() -> PathBuf {
        let home = std::env::var_os("RIGHT_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".right"));
        let p = model_cache_path(&home, WhisperModel::Tiny);
        if !p.exists() {
            download_model(WhisperModel::Tiny, &p)
                .await
                .expect("test setup: download ggml-tiny");
        }
        p
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[tokio::test]
    async fn missing_model_returns_modelmissing() {
        let engine = WhisperEngine::new(PathBuf::from("/nonexistent/ggml-tiny.bin"));
        let res = engine.transcribe(vec![0.0; 16000]).await;
        match res {
            Err(SttError::ModelMissing(_)) => {}
            other => panic!("expected ModelMissing, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn inference_returns_known_words() {
        let model = ensure_tiny_model().await;
        let samples = decode_to_pcm_f32(&fixture("hello.oga"))
            .await
            .expect("ffmpeg required");
        let engine = WhisperEngine::new(model);
        let (text, _lang) = engine
            .transcribe(samples)
            .await
            .expect("inference should succeed");
        let lower = text.to_lowercase();
        assert!(
            lower.contains("test") || lower.contains("this"),
            "expected 'this is a test' content, got: {text:?}"
        );
    }
}
