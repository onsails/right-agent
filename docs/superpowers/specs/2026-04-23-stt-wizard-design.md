# STT Wizard Onboarding Design

**Date:** 2026-04-23
**Status:** Draft

## Problem

The voice-STT feature (shipped 2026-04-23, see `2026-04-23-voice-stt-design.md`)
defaults `SttConfig.enabled = true`. Two consequences:

1. **Surprise for existing agents.** An agent created before the feature
   has no `stt:` block in `agent.yaml`. After upgrade, serde fills the
   default — STT silently turns on. On the next `rightclaw up`, a
   ~470 MB model download fires for an agent whose owner never opted in.
2. **Silent ffmpeg dependency.** STT only works when `ffmpeg` is on the
   host PATH. New users learn this only when they send their first voice
   message and see the marker `[Пользователь прислал голосовое сообщение,
   но расшифровка недоступна — на хосте не установлен ffmpeg]`. By that
   point the agent is already running.

## Goal

Make STT a deliberate, wizard-driven decision at agent init / config time,
with ffmpeg detection and an optional auto-install on macOS.

## Non-goals

- Bundling ffmpeg with the rightclaw binary.
- Auto-install on Linux (apt requires sudo, NixOS users manage packages
  declaratively — neither fits a CLI wizard's UX).
- Per-language model selection in the wizard (auto-detect handles this).
- Live progress UI for `brew install` beyond piping its native output.

## Decisions

| Decision | Choice | Reason |
|---|---|---|
| `SttConfig.enabled` serde default | Change `true` → `false` | Pre-existing agents must NOT silently enable. Wizard is the opt-in path. |
| Wizard placement | New `Step::Stt` between `Step::ChatIds` and `Step::Memory` | Voice is Telegram-adjacent; group with other Telegram-related setup. |
| Wizard ask scope | Enable y/n + model selection (`small` highlighted as default) | User wanted explicit model choice, not silent default. |
| `--yes` mode behavior | Auto-enable iff `ffmpeg_available()`, default model `small` | `--yes` = "best-case defaults"; auto-detect ffmpeg presence is the sensible default. |
| Pre-existing agents (no `stt:` block) | Default `enabled: false` (grandfather as off) | Opt-in via `agent config`, no surprise download or behavior change. |
| `agent config` STT support | Add a new "STT" entry symmetric with the wizard | Without it, opt-in for existing agents requires manual yaml edit, violating "bot-first management" convention. |
| Auto-install scope | macOS only, via `brew install ffmpeg` | brew is universal among Mac devs and runs without sudo. Linux options (apt sudo, nix declarative) don't fit. |
| Behavior when user declines install | `enabled=false`, but model selection preserved | Reflects intent: user wanted X model when they enable STT later. |

## Architecture

### Where the changes live

```
crates/rightclaw/src/agent/types.rs
   └── SttConfig: enabled default true → false (one-line change)

crates/rightclaw/src/init.rs
   ├── InitOverrides: add `stt: SttConfig` field
   └── init_agent: write `stt:` block to agent.yaml unconditionally

crates/rightclaw-cli/src/wizard.rs
   ├── stt_setup() — wizard step: ask y/n + model, run ffmpeg check
   ├── prompt_ffmpeg_install() — macOS brew install / Linux instructions
   ├── update_agent_yaml_stt() — read-modify-write helper
   └── agent_setting_menu(): add "STT" entry that delegates to stt_setup()

crates/rightclaw-cli/src/main.rs
   ├── cmd_agent_init wizard loop: insert Step::Stt
   ├── cmd_agent_init --yes path: compute stt from ffmpeg_available()
   └── cmd_agent_init saved_overrides path: preserve config.stt on --force
```

### Type changes

```rust
// crates/rightclaw/src/agent/types.rs
pub struct SttConfig {
    #[serde(default)]              // bool::default() = false
    pub enabled: bool,
    #[serde(default)]
    pub model: WhisperModel,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self { enabled: false, model: WhisperModel::default() }
    }
}
// Remove `fn default_true_stt() -> bool` helper.
```

```rust
// crates/rightclaw/src/init.rs
pub struct InitOverrides {
    // ... existing fields ...
    pub stt: SttConfig,                  // NEW
}
```

### New wizard helpers

```rust
// crates/rightclaw-cli/src/wizard.rs

/// Wizard step: ask enable/disable + model selection. Triggers ffmpeg
/// install prompt when needed. Returns Some((enabled, model)) on
/// completion, None if user pressed Esc (back to previous step).
pub fn stt_setup() -> miette::Result<Option<(bool, WhisperModel)>>;

/// macOS: detect brew, prompt to install, run, re-check.
/// Linux: print install instructions only.
/// Returns true iff ffmpeg is now available in PATH.
pub fn prompt_ffmpeg_install() -> miette::Result<bool>;

/// Read-modify-write agent.yaml: replace existing stt: block or append.
/// Same shape as existing update_agent_yaml_memory / update_agent_yaml_chat_ids.
fn update_agent_yaml_stt(path: &Path, stt: &SttConfig) -> miette::Result<()>;
```

## Data flow

### Path 1: `agent init` interactive wizard

```
rightclaw agent init <name>
  ↓
Sandbox → Network → Telegram → ChatIds → [Stt] → Memory → Done

Stt step:
  ├─ Confirm("Enable voice transcription?") default=Y
  │    ├─ n → (enabled=false, model=Small) → next
  │    ├─ Esc → back to ChatIds
  │    └─ y →
  │         ├─ Select("Choose model:", [tiny / base / small★ / medium / large-v3])
  │         │    ├─ pick → continue
  │         │    └─ Esc → re-ask "Enable voice transcription?"
  │         └─ ffmpeg_available()
  │              ├─ true  → (enabled=true, model=picked)
  │              └─ false → prompt_ffmpeg_install():
  │                   ├─ macOS + brew present:
  │                   │    Confirm("Install via 'brew install ffmpeg'?")
  │                   │    ├─ y: spawn brew (output streamed); on exit
  │                   │    │     re-check ffmpeg_available()
  │                   │    │     true  → enabled=true
  │                   │    │     false → enabled=false, warn
  │                   │    └─ n: enabled=false (model preserved)
  │                   ├─ macOS + no brew: print brew.sh URL → enabled=false
  │                   └─ Linux: print apt/nix instructions → enabled=false
  ↓
init_agent(InitOverrides { stt: SttConfig { enabled, model }, ... })
  └─ writes `stt:\n  enabled: <bool>\n  model: <model>\n` to agent.yaml
```

### Path 2: `agent init --yes`

```
rightclaw agent init <name> --yes [flags]
  ↓
Non-interactive branch:
  let stt = SttConfig {
      enabled: rightclaw::stt::ffmpeg_available(),
      model: WhisperModel::Small,
  };
  if !stt.enabled {
      eprintln!("warning: STT disabled — ffmpeg not in PATH");
  }
  ↓
init_agent with stt → yaml written
```

### Path 3: `agent config` STT toggle

```
rightclaw agent config [<name>]
  ↓
agent_setting_menu loop:
  Menu shows: "STT: on (ggml-small.bin)" or "STT: off"
  ↓
User picks "STT" entry:
  → stt_setup() — same flow as wizard
  → If Some((enabled, model)) returned:
       update_agent_yaml_stt(yaml_path, &SttConfig { enabled, model })
  → If None (Esc): no change, back to menu
  ↓
Bot picks up new config on next restart / next `rightclaw up`.
```

### Path 4: Pre-existing agent (no `stt:` block)

```
Old agent.yaml (no stt: section)
  → serde default → SttConfig { enabled: false, model: Small }
  → bot starts with stt = None → voice messages flow as files (pre-feature behavior)
  → user opts in via `rightclaw agent config` → STT entry → wizard flow
  → next `rightclaw up`: ensure_models_cached collects newly-enabled agent's model
```

### Path 5: Bot startup (unchanged code)

The bot startup logic in `crates/bot/src/lib.rs` (Task 15 of voice-STT
plan) is unchanged. Its behavior changes solely because of the new
serde default. `rightclaw up` model collection (Task 16) and doctor
checks (Task 17) likewise unchanged — they already gate on
`config.stt.enabled` and behave correctly under the new default.

## Markers (no changes)

The marker text from voice-STT spec is unchanged. `enabled: false` means
no transcription happens at all — voice messages flow as files to the
sandbox (pre-feature behavior). Markers only appear when `enabled: true`.

## Error handling

All wizard / install failures are non-fatal. Wizard completes with
`enabled: false`; user opts in later via `agent config`.

| Scenario | Behavior |
|---|---|
| `brew install ffmpeg` exit non-zero | print stderr → `enabled=false` |
| brew completed but `ffmpeg_available()` still false (PATH issue) | print "restart shell or check PATH" → `enabled=false` |
| `brew` not in PATH on macOS | print https://brew.sh URL → `enabled=false` |
| Linux | print apt/nix instructions → `enabled=false` |
| Ctrl+C during `brew install` | child receives SIGINT; we treat as failed install → `enabled=false`. No partial-state cleanup (brew manages itself). |
| User declines install prompt | `enabled=false`, **model selection preserved** in yaml |
| `update_agent_yaml_stt` parse error | propagate via `miette::Result` (real yaml corruption) |
| `update_agent_yaml_stt` write error | propagate (filesystem failure) |
| Wizard cancelled (Ctrl+C anywhere) | `inquire::InquireError::OperationCanceled` → existing wizard exit path |

**Logging:**
- INFO: successful brew install
- WARN: declined install, no brew, Linux fallback
- ERROR: brew exit-fail with stderr

**No retries** on failed install. **No version check** on ffmpeg (any
version supports `-ar -ac -f f32le`). **No** brew install in `--yes`
mode (UX contract: non-interactive = no prompts).

## Testing

**Pure logic tests:**

- `stt_config_defaults_when_missing` — update existing test from
  `assert!(cfg.stt.enabled)` to `assert!(!cfg.stt.enabled)`. The new
  default protects existing agents from surprise enablement.
- New: `pre_existing_yaml_without_stt_block_defaults_to_disabled` —
  yaml with no `stt:` block parses to `enabled=false`. Critical
  regression test.
- `stt_config_explicit_yaml_roundtrip` — unchanged (explicit values
  still work).
- New `update_agent_yaml_stt` tests:
  - **Append**: yaml without `stt:` block gets one appended; serialized
    fields match input.
  - **Replace**: yaml with existing `stt: { enabled: true, model: tiny }`
    → after call with `(false, small)` → block replaced, not duplicated.

**Excluded from automated tests:**

- `inquire`-driven UI (`stt_setup`, `prompt_ffmpeg_install`). Interactive
  prompts don't unit-test cleanly without complex mocking. Cover via
  manual smoke test below.
- Real `brew install ffmpeg`. Too destructive for CI and dev machines.

**Manual smoke test plan:**

1. `rightclaw agent init test-stt` (ffmpeg already installed) → Stt
   step → Y → small → expect yaml has `stt: { enabled: true, model: small }`.
2. `brew uninstall ffmpeg` then `rightclaw agent init test-noffmpeg` →
   Stt step → Y → install prompt → Y → wait for brew → yaml has
   `enabled: true`.
3. Same as (2) but decline install → yaml has `enabled: false, model: <picked>`.
4. `rightclaw agent init test-yes --yes` without ffmpeg → stderr WARN,
   yaml has `enabled: false`.
5. Pre-existing agent (no `stt:` block in yaml) → `rightclaw agent config`
   → menu shows "STT: off" → pick STT → wizard flow → save → yaml updated.

## Out-of-scope follow-ups

- Linux auto-install (apt sudo, nix declarative integration).
- ffmpeg version validation.
- `agent config --set stt.enabled=true` non-interactive form (current
  CLI doesn't support direct `--set`; would extend a generic mechanism).
- Re-running install prompt later if user declined.
- Wizard for `rightclaw init` (top-level project init) to set
  organization-wide STT defaults.
