# STT Wizard Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make STT a deliberate, wizard-driven decision: change `SttConfig.enabled` serde default to `false`, add a wizard step + `agent config` toggle, with macOS `brew install ffmpeg` auto-install.

**Architecture:** Default `SttConfig.enabled` flips from `true` → `false` (existing agents grandfathered as off). New `Step::Stt` in the agent-init interactive wizard between `ChatIds` and `Memory`. New `stt_setup()` helper handles ask-y/n + model select + ffmpeg detection; `prompt_ffmpeg_install()` runs `brew install ffmpeg` on macOS or prints platform-specific instructions on Linux. `agent config` menu gets a symmetric STT entry. `agent init --yes` auto-enables iff ffmpeg is on PATH.

**Tech Stack:** Rust 2024, `inquire` (already used by wizard), `tokio::process` for `brew install` subprocess, `serde-saphyr` for yaml parse/round-trip.

---

## File Structure

**Modified:**
- `crates/rightclaw/src/agent/types.rs` — `SttConfig.enabled` serde default `true` → `false`; `Default` impl updated; remove `default_true_stt()` helper.
- `crates/rightclaw/src/init.rs` — `InitOverrides` gains `stt: SttConfig` field; `init_agent()` writes `stt:` block to `agent.yaml`; existing tests updated.
- `crates/rightclaw-cli/src/wizard.rs` — new `stt_setup()`, `prompt_ffmpeg_install()`, `update_agent_yaml_stt()`; `agent_setting_menu()` gains STT entry.
- `crates/rightclaw-cli/src/main.rs` — `cmd_agent_init` interactive wizard inserts `Step::Stt`; `--yes` branch computes stt from `ffmpeg_available()`; `saved_overrides` path on `--force` preserves `config.stt`.

**No new files.**

---

## Task 1: Flip `SttConfig.enabled` serde default to `false`

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs`

- [ ] **Step 1: Update existing test + add regression test**

In `crates/rightclaw/src/agent/types.rs`, find the existing `mod stt_config_tests` (around lines ~664–707). The current `stt_config_defaults_when_missing` test asserts `cfg.stt.enabled` is true. Change the assertion:

```rust
#[test]
fn stt_config_defaults_when_missing() {
    let yaml = "";  // or "{}" — both yield AgentConfig::default()
    let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    assert!(!cfg.stt.enabled, "default must be false to grandfather existing agents");
    assert_eq!(cfg.stt.model, WhisperModel::Small);
}
```

Append a new dedicated regression test:

```rust
#[test]
fn pre_existing_yaml_without_stt_block_defaults_to_disabled() {
    // Simulates an agent.yaml from before the STT feature shipped:
    // it has other fields but no stt: block.
    let yaml = "telegram_token: \"x\"\nmodel: sonnet\n";
    let cfg: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
    assert!(
        !cfg.stt.enabled,
        "existing agents without stt: block must NOT be silently enabled"
    );
}
```

- [ ] **Step 2: Run tests — confirm one fails**

```bash
cargo test -p rightclaw stt_config_
```

Expected: `stt_config_defaults_when_missing` and `pre_existing_yaml_without_stt_block_defaults_to_disabled` FAIL because the current default is `true`.

- [ ] **Step 3: Change the serde default**

In `crates/rightclaw/src/agent/types.rs`, find the `SttConfig` struct (near line ~360) and change:

```rust
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SttConfig {
    #[serde(default)]   // bool::default() == false
    pub enabled: bool,
    #[serde(default)]
    pub model: WhisperModel,
}
```

Update the `Default` impl just below:

```rust
impl Default for SttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: WhisperModel::default(),
        }
    }
}
```

Delete the `fn default_true_stt() -> bool { true }` helper (it was the only consumer of the previous custom default).

- [ ] **Step 4: Run tests — confirm pass**

```bash
cargo test -p rightclaw stt_config_
```

Expected: all stt_config_ tests pass (including the two updated/added).

Then run full workspace check to confirm nothing else broke:

```bash
cargo check --workspace --tests
```

Expected: 0 errors, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs
git commit -m "fix(stt): default enabled=false to grandfather existing agents"
```

---

## Task 2: Add `stt: SttConfig` field to `InitOverrides`

**Files:**
- Modify: `crates/rightclaw/src/init.rs`

- [ ] **Step 1: Inspect existing `InitOverrides` and `default_overrides`**

Read `crates/rightclaw/src/init.rs` around lines 11–58 to confirm the current shape of `InitOverrides` and the `default_overrides` constant inside `init_agent()`.

- [ ] **Step 2: Add field + default**

In `crates/rightclaw/src/init.rs`, add a field to the struct definition:

```rust
pub struct InitOverrides {
    pub sandbox_mode: SandboxMode,
    pub network_policy: NetworkPolicy,
    pub telegram_token: Option<String>,
    pub allowed_chat_ids: Vec<i64>,
    pub model: Option<String>,
    pub env: HashMap<String, String>,
    pub memory_provider: MemoryProvider,
    pub memory_api_key: Option<String>,
    pub memory_bank_id: Option<String>,
    pub memory_recall_budget: RecallBudget,
    pub memory_recall_max_tokens: u32,
    pub stt: crate::agent::types::SttConfig,        // NEW
}
```

Update `default_overrides` inside `init_agent()` (around line 46):

```rust
let default_overrides = InitOverrides {
    sandbox_mode: SandboxMode::default(),
    network_policy: NetworkPolicy::default(),
    telegram_token: None,
    allowed_chat_ids: vec![],
    model: None,
    env: HashMap::new(),
    memory_provider: MemoryProvider::File,
    memory_api_key: None,
    memory_bank_id: None,
    memory_recall_budget: DEFAULT_RECALL_BUDGET,
    memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
    stt: crate::agent::types::SttConfig::default(),  // NEW
};
```

- [ ] **Step 3: Update internal test fixtures**

In `crates/rightclaw/src/init.rs` there are several `InitOverrides { ... }` literals inside `#[cfg(test)] mod tests` (around lines 1004, 1033, 1057, 1082, 1116). Add `stt: crate::agent::types::SttConfig::default(),` to each.

Use a single `git grep -n 'InitOverrides {' crates/rightclaw/src/init.rs` to find all of them.

- [ ] **Step 4: Compile**

```bash
cargo check --workspace --tests
```

Expected: 0 errors. (Existing tests should still pass — we haven't changed behavior, just added a default-equipped field.)

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/init.rs
git commit -m "feat(stt): add stt field to InitOverrides"
```

---

## Task 3: Make `init_agent()` write `stt:` block to agent.yaml

**Files:**
- Modify: `crates/rightclaw/src/init.rs`

- [ ] **Step 1: Write failing test**

Append to the `tests` module at the bottom of `crates/rightclaw/src/init.rs`:

```rust
#[test]
fn init_agent_writes_stt_block_to_yaml() {
    use crate::agent::types::{SttConfig, WhisperModel};

    let tmp = tempfile::TempDir::new().unwrap();
    let agents_parent = tmp.path();

    let overrides = InitOverrides {
        sandbox_mode: SandboxMode::default(),
        network_policy: NetworkPolicy::default(),
        telegram_token: Some("t".into()),
        allowed_chat_ids: vec![1],
        model: None,
        env: std::collections::HashMap::new(),
        memory_provider: MemoryProvider::File,
        memory_api_key: None,
        memory_bank_id: None,
        memory_recall_budget: DEFAULT_RECALL_BUDGET,
        memory_recall_max_tokens: DEFAULT_RECALL_MAX_TOKENS,
        stt: SttConfig { enabled: true, model: WhisperModel::Tiny },
    };

    let agent_dir = init_agent(agents_parent, "test-stt", Some(&overrides)).unwrap();
    let yaml = std::fs::read_to_string(agent_dir.join("agent.yaml")).unwrap();

    let cfg: crate::agent::types::AgentConfig = serde_saphyr::from_str(&yaml).unwrap();
    assert!(cfg.stt.enabled, "stt block must be written; default would be false");
    assert_eq!(cfg.stt.model, WhisperModel::Tiny);
}
```

- [ ] **Step 2: Run test — confirm failure**

```bash
cargo test -p rightclaw init_agent_writes_stt_block
```

Expected: FAIL because `init_agent` currently does not write the `stt:` block. Without it, the parsed `cfg.stt.enabled` falls back to the new serde default `false`.

- [ ] **Step 3: Inspect existing yaml-append code**

Read `crates/rightclaw/src/init.rs` starting around line 107 (the comment "Append dynamic config to agent.yaml in a single read-modify-write."). Understand how `sandbox`, `network_policy`, `telegram_token` etc. are serialized and appended.

- [ ] **Step 4: Implement `stt:` block append**

Inside the existing dynamic-yaml block in `init_agent` (right after the existing `memory:` / `attachments:` / etc. fields are written), add the STT section. Use the same string-append style as siblings:

```rust
// STT (always written so the bot's behavior is independent of the
// shipped serde default — a re-init with stt.enabled=false is meaningful
// and must not be confused with "no preference").
yaml_addition.push_str(&format!(
    "stt:\n  enabled: {}\n  model: {}\n",
    ov.stt.enabled,
    serde_yaml::to_string(&ov.stt.model)
        .map_err(|e| miette::miette!("serialize WhisperModel: {e}"))?
        .trim()
        .trim_matches('"'),
));
```

If `serde_yaml` is not a workspace dep, use `serde-saphyr` or a manual match — the `WhisperModel` variants serialize to kebab-case (`tiny`, `base`, `small`, `medium`, `large-v3`). A small private helper avoids the dep:

```rust
fn whisper_model_to_yaml(m: crate::agent::types::WhisperModel) -> &'static str {
    use crate::agent::types::WhisperModel as W;
    match m {
        W::Tiny => "tiny",
        W::Base => "base",
        W::Small => "small",
        W::Medium => "medium",
        W::LargeV3 => "large-v3",
    }
}
```

Use it:

```rust
yaml_addition.push_str(&format!(
    "stt:\n  enabled: {}\n  model: {}\n",
    ov.stt.enabled,
    whisper_model_to_yaml(ov.stt.model),
));
```

(Replace `yaml_addition` with whatever variable the existing append code uses — likely a `String` accumulator that gets written at the end. Match the existing style exactly.)

- [ ] **Step 5: Run test — confirm pass**

```bash
cargo test -p rightclaw init_agent_writes_stt_block
cargo test -p rightclaw                 # full crate to catch other breakage
cargo check --workspace --tests
```

Expected: new test passes, all other tests pass, workspace clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/init.rs
git commit -m "feat(stt): init_agent writes stt: block to agent.yaml"
```

---

## Task 4: Add `update_agent_yaml_stt` helper

**Files:**
- Modify: `crates/rightclaw-cli/src/wizard.rs`

- [ ] **Step 1: Inspect existing yaml-edit helpers**

Read `crates/rightclaw-cli/src/wizard.rs` lines 881–1067 to see how `update_agent_yaml_field`, `update_agent_yaml_memory`, etc. are shaped (regex-based replace with append fallback). Match that style.

- [ ] **Step 2: Write failing tests**

Append to the `wizard.rs` `tests` mod (or create one if absent):

```rust
#[cfg(test)]
mod stt_yaml_tests {
    use super::*;
    use rightclaw::agent::types::{SttConfig, WhisperModel};

    #[test]
    fn append_stt_when_block_missing() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "telegram_token: \"x\"\n").unwrap();

        let stt = SttConfig { enabled: true, model: WhisperModel::Small };
        update_agent_yaml_stt(tmp.path(), &stt).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.contains("stt:"));
        assert!(content.contains("enabled: true"));
        assert!(content.contains("model: small"));
    }

    #[test]
    fn replace_stt_when_block_present() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            "telegram_token: \"x\"\nstt:\n  enabled: true\n  model: tiny\n",
        ).unwrap();

        let stt = SttConfig { enabled: false, model: WhisperModel::Small };
        update_agent_yaml_stt(tmp.path(), &stt).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        // Block replaced, not duplicated:
        assert_eq!(content.matches("stt:").count(), 1, "exactly one stt: block");
        assert!(content.contains("enabled: false"));
        assert!(content.contains("model: small"));
        assert!(!content.contains("model: tiny"));
    }
}
```

- [ ] **Step 3: Run tests — confirm failure**

```bash
cargo test -p rightclaw-cli stt_yaml_tests
```

Expected: compile error — `update_agent_yaml_stt` is undefined.

- [ ] **Step 4: Implement helper**

Add to `crates/rightclaw-cli/src/wizard.rs` near the other `update_agent_yaml_*` helpers (around line 1067):

```rust
fn update_agent_yaml_stt(path: &Path, stt: &rightclaw::agent::types::SttConfig) -> miette::Result<()> {
    use std::io::Write;

    let model_str = match stt.model {
        rightclaw::agent::types::WhisperModel::Tiny => "tiny",
        rightclaw::agent::types::WhisperModel::Base => "base",
        rightclaw::agent::types::WhisperModel::Small => "small",
        rightclaw::agent::types::WhisperModel::Medium => "medium",
        rightclaw::agent::types::WhisperModel::LargeV3 => "large-v3",
    };
    let new_block = format!("stt:\n  enabled: {}\n  model: {}\n", stt.enabled, model_str);

    let original = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e:#}", path.display()))?;

    // Find an existing top-level `stt:` block — line starting with `stt:`
    // followed by a sequence of indented lines. Replace if present, else append.
    let updated = if let Some(start) = find_top_level_block_start(&original, "stt:") {
        // Determine the end of the indented body: scan until we hit a line that
        // is non-empty and not indented (= next top-level key) or EOF.
        let body_start = start + "stt:\n".len();
        let mut end = body_start;
        for line in original[body_start..].split_inclusive('\n') {
            if line.starts_with(' ') || line.starts_with('\t') || line.trim().is_empty() {
                end += line.len();
            } else {
                break;
            }
        }
        let mut out = String::with_capacity(original.len() + new_block.len());
        out.push_str(&original[..start]);
        out.push_str(&new_block);
        out.push_str(&original[end..]);
        out
    } else {
        let mut out = original;
        if !out.ends_with('\n') { out.push('\n'); }
        out.push_str(&new_block);
        out
    };

    let mut f = std::fs::File::create(path)
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;
    f.write_all(updated.as_bytes())
        .map_err(|e| miette::miette!("write {}: {e:#}", path.display()))?;
    Ok(())
}

/// Find the byte offset of a top-level `key:` line (i.e., not indented).
/// Returns None if no such line exists.
fn find_top_level_block_start(yaml: &str, key: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in yaml.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(|c: char| c.is_whitespace());
        if trimmed == line && line.starts_with(key) {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}
```

- [ ] **Step 5: Run tests — confirm pass**

```bash
cargo test -p rightclaw-cli stt_yaml_tests
cargo check --workspace --tests
```

Expected: 2 tests pass, workspace clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/wizard.rs
git commit -m "feat(stt): add update_agent_yaml_stt helper"
```

---

## Task 5: Add `prompt_ffmpeg_install` helper

**Files:**
- Modify: `crates/rightclaw-cli/src/wizard.rs`

This helper is interactive (uses `inquire::Confirm`) and shells out to `brew`, so it's not unit-testable. We compile-check, run a smoke test manually after Task 11, and rely on the spec's manual smoke test plan.

- [ ] **Step 1: Implement**

Add to `crates/rightclaw-cli/src/wizard.rs`:

```rust
/// macOS: detect brew, prompt to install ffmpeg, run, re-check.
/// Linux: print install instructions only.
/// Returns true iff ffmpeg is in PATH after this call.
pub fn prompt_ffmpeg_install() -> miette::Result<bool> {
    if rightclaw::stt::ffmpeg_available() {
        return Ok(true);
    }

    match std::env::consts::OS {
        "macos" => {
            if which::which("brew").is_err() {
                eprintln!("ffmpeg required, but Homebrew (brew) is not installed.");
                eprintln!("Install Homebrew first: https://brew.sh");
                eprintln!("Then run: brew install ffmpeg");
                return Ok(false);
            }
            let install = inquire::Confirm::new(
                "ffmpeg required for voice transcription. Install via 'brew install ffmpeg' (~50 MB, ~30 sec)?"
            )
            .with_default(true)
            .prompt()
            .map_err(|e| miette::miette!("prompt failed: {e:#}"))?;
            if !install {
                eprintln!("STT will be disabled. Install ffmpeg later: brew install ffmpeg");
                return Ok(false);
            }
            // Spawn brew install with stdout/stderr inherited so user sees output.
            let status = std::process::Command::new("brew")
                .args(["install", "ffmpeg"])
                .status()
                .map_err(|e| miette::miette!("spawn brew: {e:#}"))?;
            if !status.success() {
                eprintln!("brew install ffmpeg exited with {status}; STT disabled.");
                return Ok(false);
            }
            if !rightclaw::stt::ffmpeg_available() {
                eprintln!(
                    "brew completed but ffmpeg not yet in PATH — restart shell or check PATH; STT disabled."
                );
                return Ok(false);
            }
            tracing::info!("ffmpeg installed via brew");
            Ok(true)
        }
        "linux" => {
            eprintln!("ffmpeg required for voice transcription. Install:");
            eprintln!("  Debian/Ubuntu:  sudo apt install ffmpeg");
            eprintln!("  NixOS / devenv: add 'pkgs.ffmpeg' to your packages");
            eprintln!("Then re-run this command.");
            Ok(false)
        }
        other => {
            eprintln!("ffmpeg required, but auto-install is not supported on '{other}'.");
            eprintln!("Install ffmpeg from https://ffmpeg.org/download.html, then re-run.");
            Ok(false)
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check --workspace --tests
```

Expected: 0 errors, 0 warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/wizard.rs
git commit -m "feat(stt): add prompt_ffmpeg_install with macOS brew auto-install"
```

---

## Task 6: Add `stt_setup` wizard helper

**Files:**
- Modify: `crates/rightclaw-cli/src/wizard.rs`

Like Task 5, this is interactive and not unit-testable. Compile-check + manual smoke test cover it.

- [ ] **Step 1: Implement**

Add to `crates/rightclaw-cli/src/wizard.rs`:

```rust
/// Wizard step: ask enable/disable + model selection, run ffmpeg detection
/// + install prompt as needed. Returns Some((enabled, model)) on completion,
/// None if the user pressed Esc on the first prompt (back to previous step).
pub fn stt_setup() -> miette::Result<Option<(bool, rightclaw::agent::types::WhisperModel)>> {
    use rightclaw::agent::types::WhisperModel;

    // Step 1: enable y/n
    let enable = match inquire::Confirm::new("Enable voice transcription?")
        .with_default(true)
        .with_help_message(
            "Telegram voice messages and video notes will be transcribed locally via whisper.cpp.",
        )
        .prompt()
    {
        Ok(v) => v,
        Err(inquire::InquireError::OperationCanceled)
        | Err(inquire::InquireError::OperationInterrupted) => return Ok(None),
        Err(e) => return Err(miette::miette!("prompt failed: {e:#}")),
    };

    if !enable {
        return Ok(Some((false, WhisperModel::Small)));
    }

    // Step 2: model select
    let options = vec![
        "tiny     — ~75 MB,   fastest, OK for short commands",
        "base     — ~150 MB,  decent",
        "small    — ~470 MB,  recommended (default)",
        "medium   — ~1.5 GB,  very good",
        "large-v3 — ~3.0 GB,  best quality, slow",
    ];
    let picked = match inquire::Select::new("Choose whisper model:", options.clone())
        .with_starting_cursor(2) // small
        .prompt()
    {
        Ok(v) => v,
        Err(inquire::InquireError::OperationCanceled)
        | Err(inquire::InquireError::OperationInterrupted) => {
            // Back up to "Enable?" — caller's loop will re-enter this fn.
            return Ok(None);
        }
        Err(e) => return Err(miette::miette!("prompt failed: {e:#}")),
    };
    let model = if picked.starts_with("tiny") {
        WhisperModel::Tiny
    } else if picked.starts_with("base") {
        WhisperModel::Base
    } else if picked.starts_with("small") {
        WhisperModel::Small
    } else if picked.starts_with("medium") {
        WhisperModel::Medium
    } else {
        WhisperModel::LargeV3
    };

    // Step 3: ffmpeg check + optional install
    let ffmpeg_ok = prompt_ffmpeg_install()?;
    Ok(Some((ffmpeg_ok, model)))
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check --workspace --tests
```

Expected: 0 errors, 0 warnings.

- [ ] **Step 3: Commit**

```bash
git add crates/rightclaw-cli/src/wizard.rs
git commit -m "feat(stt): add stt_setup wizard helper"
```

---

## Task 7: Insert `Step::Stt` into `cmd_agent_init` interactive wizard

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Locate the wizard loop**

Open `crates/rightclaw-cli/src/main.rs` around lines 1572–1680 (the `loop { match step { Step::Sandbox => ..., Step::Network => ..., ..., Step::Done => break, } }` body inside `cmd_agent_init`).

- [ ] **Step 2: Add `Step::Stt` variant + state**

Modify the inline `enum Step` at line ~1573:

```rust
#[derive(Clone, Copy)]
enum Step {
    Sandbox,
    Network,
    Telegram,
    ChatIds,
    Stt,        // NEW
    Memory,
    Done,
}
```

Add a state variable next to `w_token`, `w_chat_ids`, `w_mem`:

```rust
let mut w_stt: rightclaw::agent::types::SttConfig =
    rightclaw::agent::types::SttConfig::default();
```

- [ ] **Step 3: Insert step handling**

In the `match step { ... }` body, the existing flow goes Sandbox → Network → Telegram → ChatIds → Memory → Done.

Wire `Stt` between `ChatIds` and `Memory`:

In the existing `Step::ChatIds` arm, change `step = Step::Memory` to `step = Step::Stt`.

Insert a new arm `Step::Stt`:

```rust
Step::Stt => match crate::wizard::stt_setup() {
    Ok(Some((enabled, model))) => {
        w_stt = rightclaw::agent::types::SttConfig { enabled, model };
        step = Step::Memory;
    }
    Ok(None) => {
        // Back to the previous step (ChatIds if Telegram was set, else Telegram).
        step = if w_token.is_some() { Step::ChatIds } else { Step::Telegram };
    }
    Err(e) => return Err(e),
},
```

In the existing `Step::Memory` arm, the `None` (back) branch should now go back to `Step::Stt`:

```rust
Step::Memory => match rightclaw::init::prompt_memory_config(name)? {
    Some((p, k, b, rb, rt)) => {
        w_mem = (p, k, b, rb, rt);
        step = Step::Done;
    }
    None => {
        step = Step::Stt;          // CHANGED from prior chain
    }
},
```

- [ ] **Step 4: Wire `w_stt` into the constructed `InitOverrides`**

After the loop (around line 1667), the existing `InitOverrides { ... }` literal needs the new field:

```rust
rightclaw::init::InitOverrides {
    sandbox_mode: w_sandbox,
    network_policy: w_network,
    telegram_token: w_token,
    allowed_chat_ids: w_chat_ids,
    model: None,
    env: std::collections::HashMap::new(),
    memory_provider: w_mem.0,
    memory_api_key: w_mem.1,
    memory_bank_id: w_mem.2,
    memory_recall_budget: w_mem.3,
    memory_recall_max_tokens: w_mem.4,
    stt: w_stt,                     // NEW
}
```

- [ ] **Step 5: Compile**

```bash
cargo check --workspace --tests
```

Expected: 0 errors. If any other `InitOverrides { ... }` literal in `main.rs` fails to compile, add `stt: rightclaw::agent::types::SttConfig::default()` to it.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat(stt): add Step::Stt to agent init interactive wizard"
```

---

## Task 8: Update `cmd_agent_init --yes` non-interactive path

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Locate the non-interactive branch**

In `crates/rightclaw-cli/src/main.rs` around line 1555, find the `if !interactive { rightclaw::init::InitOverrides { ... } }` block.

- [ ] **Step 2: Compute and inject `stt`**

Replace the current `InitOverrides` literal with:

```rust
if !interactive {
    let ffmpeg_ok = rightclaw::stt::ffmpeg_available();
    let stt = rightclaw::agent::types::SttConfig {
        enabled: ffmpeg_ok,
        model: rightclaw::agent::types::WhisperModel::Small,
    };
    if !ffmpeg_ok {
        eprintln!(
            "warning: STT disabled — ffmpeg not in PATH. \
             Install (macOS): brew install ffmpeg, then re-run with --force."
        );
    }
    rightclaw::init::InitOverrides {
        sandbox_mode: sandbox_mode
            .unwrap_or(rightclaw::agent::types::SandboxMode::Openshell),
        network_policy: network_policy
            .unwrap_or(rightclaw::agent::types::NetworkPolicy::Permissive),
        telegram_token: None,
        allowed_chat_ids: vec![],
        model: None,
        env: std::collections::HashMap::new(),
        memory_provider: rightclaw::agent::types::MemoryProvider::Hindsight,
        memory_api_key: None,
        memory_bank_id: None,
        memory_recall_budget: rightclaw::init::DEFAULT_RECALL_BUDGET,
        memory_recall_max_tokens: rightclaw::init::DEFAULT_RECALL_MAX_TOKENS,
        stt,
    }
} else {
    // ... existing interactive branch ...
}
```

- [ ] **Step 3: Compile**

```bash
cargo check --workspace --tests
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat(stt): agent init --yes auto-enables STT iff ffmpeg in PATH"
```

---

## Task 9: Preserve `config.stt` in `cmd_agent_init` saved_overrides path

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs`

- [ ] **Step 1: Locate the saved_overrides branch**

In `crates/rightclaw-cli/src/main.rs` around line 1498, find the `if let Some(config) = saved_overrides { rightclaw::init::InitOverrides { ... } }` block. This is the path used by `--force` re-init when not `--fresh`.

- [ ] **Step 2: Add the `stt` field**

Add `stt: config.stt,` to the `InitOverrides` literal so existing STT config survives a `--force` re-init:

```rust
let overrides = if let Some(config) = saved_overrides {
    rightclaw::init::InitOverrides {
        sandbox_mode: config.sandbox_mode().clone(),
        network_policy: config.network_policy,
        telegram_token: config.telegram_token,
        allowed_chat_ids: config.allowed_chat_ids,
        model: config.model,
        env: config.env,
        memory_provider: config
            .memory
            .as_ref()
            .map(|m| m.provider.clone())
            .unwrap_or_default(),
        memory_api_key: config.memory.as_ref().and_then(|m| m.api_key.clone()),
        memory_bank_id: config.memory.as_ref().and_then(|m| m.bank_id.clone()),
        memory_recall_budget: config
            .memory
            .as_ref()
            .map(|m| m.recall_budget.clone())
            .unwrap_or(rightclaw::init::DEFAULT_RECALL_BUDGET),
        memory_recall_max_tokens: config
            .memory
            .as_ref()
            .map(|m| m.recall_max_tokens)
            .unwrap_or(rightclaw::init::DEFAULT_RECALL_MAX_TOKENS),
        stt: config.stt,                // NEW
    }
} else {
    // ... unchanged ...
};
```

- [ ] **Step 3: Compile**

```bash
cargo check --workspace --tests
```

Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat(stt): preserve stt config across --force re-init"
```

---

## Task 10: Add STT entry to `agent_setting_menu`

**Files:**
- Modify: `crates/rightclaw-cli/src/wizard.rs`

- [ ] **Step 1: Locate the menu construction**

In `crates/rightclaw-cli/src/wizard.rs` around lines 575–610, find the `let opt_token = ...; let opt_model = ...; ...` block that builds menu entries inside `agent_setting_menu`.

- [ ] **Step 2: Add STT display + entry + select branch**

Compute the display string after the existing displays (around line ~577):

```rust
let stt_display = if config.stt.enabled {
    format!("on ({})", whisper_model_to_yaml_str(config.stt.model))
} else {
    "off".to_string()
};
```

Where `whisper_model_to_yaml_str` is a small helper at the top of the module (or duplicated from Task 4 — DRY by extracting it):

```rust
fn whisper_model_to_yaml_str(m: rightclaw::agent::types::WhisperModel) -> &'static str {
    use rightclaw::agent::types::WhisperModel as W;
    match m {
        W::Tiny => "tiny",
        W::Base => "base",
        W::Small => "small",
        W::Medium => "medium",
        W::LargeV3 => "large-v3",
    }
}
```

If the same helper was added to `update_agent_yaml_stt` in Task 4, share it (extract to module scope so both call sites use one definition).

Add the menu entry next to other `opt_*` declarations (around line 583):

```rust
let opt_stt = format!("STT: {stt_display}");
```

Push it into `options` (after `opt_memory`, before `opt_done`):

```rust
options.push(opt_stt.clone());
```

In the `match` block that handles selection (further down — find the existing `if choice == opt_token { ... } else if choice == opt_model { ... } ...` chain), add a new arm:

```rust
} else if choice == opt_stt {
    if let Some((enabled, model)) = stt_setup()? {
        let stt = rightclaw::agent::types::SttConfig { enabled, model };
        update_agent_yaml_stt(&agent_yaml_path, &stt)?;
    }
    // None = user cancelled, leave config unchanged.
}
```

- [ ] **Step 3: Compile**

```bash
cargo check --workspace --tests
```

Expected: 0 errors.

- [ ] **Step 4: Verify existing yaml-write tests still pass**

```bash
cargo test -p rightclaw-cli stt_yaml_tests
cargo test -p rightclaw-cli                 # broader sanity
```

Expected: 2/2 stt_yaml_tests pass; nothing else regressed.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw-cli/src/wizard.rs
git commit -m "feat(stt): add STT entry to agent config menu"
```

---

## Task 11: Final verification + manual smoke test

**No file edits in this task.** Verify the integrated feature.

- [ ] **Step 1: Full workspace build + test**

```bash
cargo check --workspace --tests
cargo test --workspace
```

Expected: 0 errors, 0 warnings; all tests pass (the prior 22 stt tests from voice-STT plan + new stt_config_ + init_agent_writes_stt_block + stt_yaml_tests + everything else).

- [ ] **Step 2: Manual smoke test (per spec § Testing)**

Run these on a dev machine. Document any deviation as a follow-up TODO:

1. **Wizard, ffmpeg present.** `rightclaw agent init test-stt-1` (assume ffmpeg already installed). At Stt step: `Y` → `small`. Verify `agents/test-stt-1/agent.yaml` contains:
   ```yaml
   stt:
     enabled: true
     model: small
   ```
2. **Wizard, ffmpeg missing, install accepted (macOS).** `brew uninstall ffmpeg`, then `rightclaw agent init test-stt-2`. At Stt step: `Y` → `small` → install prompt → `Y`. Wait for brew. Verify `enabled: true` in yaml.
3. **Wizard, ffmpeg missing, install declined.** Same as (2) but `n` on install prompt. Verify `enabled: false, model: small` in yaml (model preserved).
4. **--yes, no ffmpeg.** `brew uninstall ffmpeg`, then `rightclaw agent init test-stt-3 --yes`. Verify stderr WARN, yaml `enabled: false`.
5. **Existing agent opt-in.** Pick an agent that has no `stt:` block in its `agent.yaml` (or temporarily delete it from a test agent's yaml). Run `rightclaw agent config <name>`. Menu shows `STT: off`. Pick STT → `Y` → `small`. Verify yaml has `enabled: true` after save, only one `stt:` block.

Each scenario passing means the corresponding code path is verified.

- [ ] **Step 3: No commit (verification only)**

If anything failed in Step 2, file follow-up tasks; do not paper over with extra commits in this task.

---

## Self-review summary

- **Spec coverage** — every Decision row in the spec maps to a task:
  - `enabled` default true → false: Task 1.
  - Wizard placement (between ChatIds & Memory): Task 7.
  - Wizard ask scope (y/n + model): Tasks 5–6 (helpers) + Task 7 (wiring).
  - `--yes` auto-enable iff ffmpeg: Task 8.
  - Pre-existing agents default false: Task 1 (regression test).
  - `agent config` STT support: Task 10.
  - Auto-install scope macOS only: Task 5.
  - Decline-install preserves model: implemented in Task 6 (returns `(false, model)`).
- **Placeholder scan** — no TBD/TODO/"implement later" tokens. Every code step has the actual code.
- **Type consistency** — `SttConfig`, `WhisperModel`, `InitOverrides`, `agent_setting_menu`, `stt_setup`, `update_agent_yaml_stt`, `prompt_ffmpeg_install` referenced consistently across tasks. The `whisper_model_to_yaml`/`whisper_model_to_yaml_str` helper is intentionally duplicated to avoid cross-crate plumbing; consolidation is noted as a follow-up.
- **Testing** — automated tests cover all the unit-testable surfaces (serde defaults, yaml regression, helper read-modify-write). Interactive helpers covered by the manual smoke plan as the spec specifies.
