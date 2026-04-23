# Voice & Video-Note Transcription (STT) Design

**Date:** 2026-04-23
**Status:** Draft

## Problem

When a user sends a Telegram voice message (`voice`, OGG/Opus) or a video note
(`video_note`, MP4 "кружок"), the bot currently downloads the file and uploads
it to the agent's sandbox inbox as an attachment. Claude has no audio
transcription capability, so the agent sees an `.oga` / `.mp4` file it cannot
read and replies with something like "I can't transcribe voice messages."

The Anthropic API does not accept audio input. To make voice work, the bot must
transcribe audio to text **before** the payload reaches Claude.

## Goal

Recognize voice messages and circular video notes on the host (in the bot),
substitute the transcription into the user-facing payload, and let the agent
respond as if the user had typed the text — without depending on any external
service.

## Non-goals

- Music / podcast / lecture transcription (Telegram `audio` type) — out of
  scope for v1. These are different workflows (long-form, agent may want to
  do something other than transcribe). They continue to be uploaded as files.
- Video transcription (Telegram `video` type) — out of scope. Files can be
  huge and the workflow is not "user said something."
- Cloud STT providers (Groq, OpenAI Whisper API). User explicitly chose not
  to depend on external services.
- Translation. Whisper can translate to English, but we want verbatim
  transcript in the original language.
- Multilingual UI: error markers are Russian-only for now. If we ever ship
  English-speaking agents, we extract markers into constants.

## Decisions

| Decision | Choice | Reason |
|---|---|---|
| STT location | Host-side, in `rightclaw-bot` | Bot is on host. Sandbox-side STT means per-agent ~700 MB footprint, slow CPU-only inference, and policy bloat. |
| STT engine | `whisper-rs` (Rust binding to whisper.cpp) | Pure-Rust integration, Metal acceleration on macOS, no Python in the host stack. |
| Default model | `small` (multilingual, ~470 MB) | Noticeably better than `base` for Russian; fast enough on M-series with Metal. |
| Per-agent override | Yes, from day one | Some agents may want `tiny` for speed or `medium` for quality. |
| Audio decoding | System `ffmpeg` (subprocess, stdout pipe) | Pure-Rust opus/aac decoding (symphonia + rubato) is unproven for our inputs; ffmpeg is a de-facto standard like git. |
| Missing-ffmpeg behavior | Fail soft: bot runs, voices return an error marker visible to the user via the agent | User sees the problem in chat, can install ffmpeg, no silent failure. |
| Model storage | `~/.rightclaw/cache/whisper/ggml-{model}.bin` | Shared across agents using the same model. |
| Model download timing | At `rightclaw up`, only if ffmpeg is present | Predictable, no first-voice latency. Don't waste bandwidth on a model that can't be used. |
| Attachment scope | `voice` + `video_note` | Both are "user said something" semantically. |
| Payload format | Marker-wrapped transcript injected into user text. Original audio file dropped. | Hermes-style; agent knows the source was voice; sandbox stays clean. |
| Config | `stt: { enabled: bool, model: enum }` in `agent.yaml` | YAGNI on language and max-duration — auto-detect handles language; voice/video_note size limits handle duration. |

## Architecture

### Where STT lives

A new module in the bot crate:

```
crates/bot/src/stt/
├── mod.rs        # Public API: Transcriber, SttError, TranscriptionResult
├── model.rs      # WhisperModel enum, filenames, download URLs, cache paths
├── decode.rs     # ffmpeg subprocess: file path → Vec<f32> PCM 16kHz mono
└── whisper.rs    # whisper-rs wrapper: lazy WhisperContext, inference
```

### Component types

```rust
// crates/bot/src/stt/mod.rs
pub struct Transcriber {
    model_path: PathBuf,
    ctx: OnceCell<Arc<Mutex<WhisperContext>>>,  // lazy init on first call
}

pub struct TranscriptionResult {
    pub text: String,
    pub duration_ms: u64,
    pub detected_language: Option<String>,
}

#[derive(Debug, thiserror::Error)]
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

impl Transcriber {
    pub fn new(model_path: PathBuf) -> Self;
    pub async fn transcribe_voice(&self, file: &Path) -> Result<TranscriptionResult, SttError>;
    pub async fn transcribe_video_note(&self, file: &Path) -> Result<TranscriptionResult, SttError>;
}
```

`transcribe_voice` and `transcribe_video_note` share the same internal
implementation (`transcribe_inner`); the public split exists for clarity in
logs and metrics.

### Configuration schema (`agent.yaml`)

```yaml
stt:
  enabled: true        # default: true (backward-compatible for existing agents)
  model: small         # default: small
                       # values: tiny | base | small | medium | large-v3
```

Reflected in `crates/rightclaw/src/agent/types.rs`:

```rust
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct SttConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub model: WhisperModel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default,
         serde::Deserialize, serde::Serialize)]
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
    pub fn filename(&self) -> &'static str;       // "ggml-small.bin"
    pub fn download_url(&self) -> &'static str;   // huggingface URL
    pub fn approx_size_mb(&self) -> u64;          // for progress UI
}
```

`AgentConfig` gains `pub stt: SttConfig` with `#[serde(default)]`.

### Cargo dependencies (`crates/bot/Cargo.toml`)

```toml
whisper-rs = { version = "0.x", default-features = false }
# Metal feature only on macOS; resolved via target_os cfg in code or features
```

(Exact version resolved when implementing; bot crate already pulls tokio, anyhow,
thiserror.)

## Data flow

### End-to-end: user sends a voice message

```
Telegram                Bot                                      Sandbox
   │                     │                                          │
   │  voice .oga         │                                          │
   ├────────────────────►│                                          │
   │                     │ dispatch → handler → worker (debounce 500ms)
   │                     │ extract_attachments() → [Voice]          │
   │  GET file           │                                          │
   │◄────────────────────┤                                          │
   ├────────────────────►│ download → tmp/inbox/<id>.oga            │
   │                     │                                          │
   │                     │ ┌── Transcriber ─────────────────────┐   │
   │                     │ │ ffmpeg -nostdin -loglevel error    │   │
   │                     │ │   -i <oga> -ar 16000 -ac 1         │   │
   │                     │ │   -f f32le pipe:1                  │   │
   │                     │ │ read stdout → Vec<f32>             │   │
   │                     │ │ spawn_blocking:                    │   │
   │                     │ │   WhisperContext (lazy init)       │   │
   │                     │ │   state.full(samples) → text       │   │
   │                     │ └────────────────────────────────────┘   │
   │                     │                                          │
   │                     │ remove .oga from upload-list             │
   │                     │ payload text =                           │
   │                     │   "[Пользователь надиктовал ... \"X\"]"  │
   │                     │                                          │
   │                     │ upload remaining attachments             │
   │                     ├─────────────────────────────────────────►│ inbox/
   │                     │                                          │
   │                     │ claude -p < payload                      │
   │                     ├─────────────────────────────────────────►│ CC sees marker
   │                     │ ◄──── reply JSON ───────────────────────┤
   │  send reply         │                                          │
   │◄────────────────────┤                                          │
```

### Decoding without a temp WAV

`ffmpeg` writes raw PCM f32 little-endian to stdout; the bot reads it into
`Vec<f32>` and feeds whisper-rs directly. No intermediate file in
`~/.rightclaw/tmp/` for the PCM stage.

```
ffmpeg -nostdin -loglevel error -i <input> -ar 16000 -ac 1 -f f32le pipe:1
```

### Lazy whisper context

First voice message in the bot's lifetime triggers `WhisperContext::new(model_path)`
(~500 ms on M-series for `small`). Subsequent voices reuse it.

Inference itself is sync; we run it via `tokio::task::spawn_blocking`. The
`Arc<Mutex<WhisperContext>>` serializes concurrent voices — fine because:
inference on one model is GPU/CPU-bound, parallel voices wouldn't help, and
voice traffic is low.

### Worker integration

In `crates/bot/src/telegram/worker.rs`, after Telegram download but before
sandbox upload:

1. If `config.stt.enabled == false` → skip; `voice` and `video_note`
   attachments upload to sandbox as today (current behavior preserved).
2. Otherwise, for each `Voice` and `VideoNote` attachment in the batch:
   - Call `transcriber.transcribe_voice(file)` /
     `transcriber.transcribe_video_note(file)`.
   - On success: drop the file from upload-list; append marker to text payload.
   - On error: drop the file from upload-list; append error marker to text
     payload.
3. Other attachments (photo, document, video, audio, sticker, animation) —
   unchanged.

The transformation is implemented as a pure helper:

```rust
async fn apply_stt(
    attachments: Vec<DownloadedAttachment>,
    user_text: Option<String>,
    transcriber: Option<Arc<Transcriber>>,
) -> (Vec<DownloadedAttachment>, Option<String>);
```

Pure-function shape makes worker integration testable without Telegram or
sandbox.

### Bot startup

In `crates/bot/src/lib.rs` after `AgentConfig` is loaded:

```
if config.stt.enabled:
    transcriber = Some(Arc::new(Transcriber::new(model_cache_path(model))))
else:
    transcriber = None
```

The bot does not fail if the model file is absent or ffmpeg is missing —
those become runtime errors surfaced via markers.

### `rightclaw up` flow

```
1. Discover agents (existing).
2. Check ffmpeg in PATH:
     if absent:
         WARN "ffmpeg not found — voice transcription disabled.
               Install: brew install ffmpeg / apt install ffmpeg.
               Skipping whisper model download."
         skip step 3 entirely.
3. Collect distinct (model) across agents where stt.enabled.
   For each model:
     path = ~/.rightclaw/cache/whisper/ggml-{model}.bin
     if path absent:
         download to path.partial with progress;
         atomic rename to path on success;
         on download failure: WARN, do not abort `up`.
4. Generate process-compose.yaml (existing).
5. Launch process-compose (existing).
```

Implemented in `crates/rightclaw/src/codegen/pipeline.rs` (or a new
`crates/rightclaw/src/stt_setup.rs` if pipeline.rs grows too large).

## Markers

All markers are Russian (agent operates in Russian). Marker text is constant;
no templating beyond the transcript / error reason.

Each marker has a voice variant and a video-note variant. The implementation
holds them as constants and selects by attachment kind.

| Case | Voice variant | Video-note variant |
|---|---|---|
| Success | `[Пользователь надиктовал голосовое сообщение. Расшифровка: "<text>"]` | `[Пользователь записал кружок (видео-сообщение). Расшифровка: "<text>"]` |
| `FfmpegNotFound` | `[Пользователь прислал голосовое сообщение, но расшифровка недоступна — на хосте не установлен ffmpeg.]` | `[Пользователь прислал кружок (видео-сообщение), но расшифровка недоступна — на хосте не установлен ffmpeg.]` |
| `ModelMissing` | `[Пользователь прислал голосовое сообщение, но модель распознавания речи не загружена. Запусти 'rightclaw up' заново.]` | `[Пользователь прислал кружок (видео-сообщение), но модель распознавания речи не загружена. Запусти 'rightclaw up' заново.]` |
| `FileTooLarge` | `[Пользователь прислал голосовое сообщение, но оно слишком большое для расшифровки (NN MB).]` | `[Пользователь прислал кружок (видео-сообщение), но он слишком большой для расшифровки (NN MB).]` |
| Other (`FfmpegFailed`, `WhisperLoadFailed`, `WhisperInferenceFailed`) | `[Пользователь прислал голосовое сообщение, но расшифровать не удалось: <короткая причина>]` | `[Пользователь прислал кружок (видео-сообщение), но расшифровать не удалось: <короткая причина>]` |

If the user's message also contained text, payload becomes:
```
<marker>

<user text>
```
If text is absent, payload is just `<marker>`.

## Error handling

All STT failures are **non-fatal for the bot**. Bot always produces a payload,
agent always responds, user always sees something in chat.

**No retries** inside STT — it's not networked, retry is meaningless.
**No fallback to cloud APIs** — violates the "no external services" principle.
**No persistence** of failed voices.

`MAX_AUDIO_FILE_MB = 25` — matches Whisper API's traditional limit and protects
against pathological inputs. Telegram voice is normally <1 MB, so this is a
guardrail, not a frequent gate.

### Logging

- `INFO transcription started kind=voice file=<path> bytes=<n>`
- `INFO transcription complete duration_ms=<n> chars=<n> language=<detected>`
- `ERROR` for `FfmpegFailed` / `WhisperLoadFailed` / `WhisperInferenceFailed`
  with full stderr / context for debugging.
- `WARN` for `ModelMissing` / `FfmpegNotFound` — these are user-environment
  issues, not bot bugs.

### Partial download recovery

`rightclaw up` writes models as `<path>.partial`, then atomically renames to
`<path>` on completion. Cache check is "does `<path>` exist" — partial files
are ignored. Interrupted downloads (Ctrl+C during `up`) leave a `.partial`
that gets overwritten on next attempt.

## Doctor checks

`crates/rightclaw/src/doctor.rs` gains:

- ffmpeg in PATH — WARN if absent (not FAIL — bot still runs).
- Per agent with `stt.enabled` — model file present? WARN if absent with
  hint "will be downloaded on next `rightclaw up`".

Doctor does not download models or install ffmpeg — only reports.

## Testing

**Approach:** no `#[ignore]` (per project rule). If ffmpeg is missing on the
dev machine, tests fail with a clear "install ffmpeg" message — same pattern
as OpenShell tests.

**Test model:** `ggml-tiny` (~75 MB), not `small`. Tests verify pipeline
correctness, not WER. Tiny is fast and cheap to download.

**Shared cache:** test model lives at `~/.rightclaw/cache/whisper/ggml-tiny.bin`,
shared across test runs. First run downloads; subsequent runs reuse.

**Audio fixtures:** committed under `crates/bot/tests/fixtures/`:
- `hello.oga` — short Telegram-style voice (~2 seconds, ~10–20 KB) with known
  content.
- `circle.mp4` — short circular video note (~20–50 KB) with known content.

### Unit tests (no ffmpeg, no whisper)

- `agent::types`: `stt` defaults, YAML round-trip, invalid model name errors.
- `stt::model`: filename, download URL, partial-file exclusion from cache check.

### Integration tests (require ffmpeg + tiny model)

- `stt::decode`: decode `hello.oga` → samples > 0; decode `circle.mp4` →
  samples > 0; corrupted input → `FfmpegFailed` with stderr; clean PATH →
  `FfmpegNotFound`.
- `stt::whisper`: inference on `hello.oga` returns text containing the known
  word; lazy context init (first call slower than second by an observable
  margin); two concurrent inferences serialize on the mutex.
- `stt::Transcriber` end-to-end: `transcribe_voice`, `transcribe_video_note`,
  missing model returns `ModelMissing`, oversized input returns `FileTooLarge`.

### Worker integration (pure function, no Telegram, no sandbox)

`apply_stt` tests:
- voice attachment replaced by marker, file dropped from upload list;
- video_note attachment replaced by marker;
- voice + user text → marker prepended above user text with blank line;
- voice without text → only marker;
- photo / document / other unchanged;
- `stt.enabled = false` → voice passes through to upload list;
- `FfmpegNotFound` → error marker emitted, file still dropped.

### `rightclaw up` integration

- Up skips model download when ffmpeg is missing.
- Up collects unique models across agents (one download per distinct model).
- Up resumes after partial download (a leftover `*.partial` is overwritten,
  not used as a cache hit).

### Doctor

- Doctor warns on missing ffmpeg.
- Doctor warns on missing models for agents with `stt.enabled`.
- Doctor stays silent for agents with `stt.enabled = false`.

### What we don't test

- whisper-rs / whisper.cpp internals (external dep, has its own tests).
- WER / quality.
- Real HuggingFace download in CI (mock HTTP).
- Real Telegram dispatch (covered indirectly by extracting `apply_stt`).

## Out-of-scope follow-ups

- `audio` (mp3/m4a) attachments — distinct workflow (long-form), revisit if
  there's user demand.
- Streaming transcription (chunked inference) — only matters for very long
  audio; voice messages don't need it.
- Speaker diarization, timestamps, word-level alignment.
- Cloud STT fallback.
- Marker localization (English / other languages).
- Per-agent language hint (`stt.language`) — auto-detect handles ru/en mix
  fine in practice.
