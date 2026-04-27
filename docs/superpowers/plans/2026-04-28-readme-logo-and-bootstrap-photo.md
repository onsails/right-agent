# README Logo + Bootstrap Welcome Photo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `# Right Agent` H1 in `README.md` with the canonical brand-guide L1 lockup, and have the Telegram bot attach a static character-mark PNG to the agent's *first* reply during a bootstrap session.

**Architecture:** Two new committed assets at repo-root `assets/`. README points at the SVG. Bot embeds the PNG at compile time via `include_bytes!` and sends it via `bot.send_photo` immediately *before* the text reply, gated by a tiny `should_send_bootstrap_photo(bootstrap_mode, is_first_call)` predicate. `is_first_call` is already a local in `invoke_cc`; we only need to thread it out to the worker reply branch where the send happens.

**Tech Stack:** SVG (assets), Rust (bot crate `right-bot`), teloxide ≥0.13, tokio.

---

## File map

**New files:**
- `assets/lockup-horizontal.svg` — README header lockup, ~360×80, mark + wordmark, all glyphs as `<path>` (no `<text>`).
- `assets/character-on-coal.svg` — character C1 mark on a `#0f0f0f` circle, viewBox `0 0 100 100`.
- `assets/character-on-coal.png` — 1024×1024 raster of the above. Binary, committed.
- `crates/bot/src/telegram/bootstrap_photo.rs` — single small module: PNG bytes constant, predicate, helper `send_if_needed`, unit tests.

**Modified files:**
- `README.md` — drop the H1, insert the centered lockup image.
- `crates/bot/src/telegram/mod.rs` — `pub(crate) mod bootstrap_photo;`.
- `crates/bot/src/telegram/worker.rs` — refactor `invoke_cc` return shape to expose `is_first_call`; call the new helper in the success branch.

**Untouched:** `BOOTSTRAP.md`, prompt assembly, MCP, schemas, agent codegen, sandbox sync.

---

### Task 1: Add the lockup SVG and character SVG/PNG assets

**Files:**
- Create: `assets/lockup-horizontal.svg`
- Create: `assets/character-on-coal.svg`
- Create: `assets/character-on-coal.png` (binary)

The wordmark in the lockup must be `<path>` data (not `<text>`), because GitHub's README image renderer does not execute embedded `@font-face` declarations. The character SVG can be hand-written from the brand-guide paths; only the wordmark needs a font-to-paths conversion.

- [ ] **Step 1: Create the assets directory**

```bash
mkdir -p assets
```

- [ ] **Step 2: Build the character-on-coal SVG by hand**

Geometry copied verbatim from `docs/brand-guidelines.html` line 376–388 (character C1 thin smile), wrapped in a coal circle.

Write `assets/character-on-coal.svg`:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="1024" height="1024">
  <circle cx="50" cy="50" r="50" fill="#0f0f0f"/>
  <path d="M14 42 L48 22 Q56 18 62 22 L84 36" stroke="#E8632A" stroke-width="8.5" stroke-linecap="round" fill="none"/>
  <path d="M14 58 L48 78 Q56 82 62 78 L84 64" stroke="#E8632A" stroke-width="8.5" stroke-linecap="round" fill="none"/>
  <rect x="8" y="38" width="11" height="24" rx="5.5" fill="#E8632A"/>
  <circle cx="42" cy="45" r="5" fill="#fff"/>
  <circle cx="42.8" cy="45" r="2.5" fill="#161616"/>
  <circle cx="44" cy="43.8" r="0.9" fill="#fff"/>
  <circle cx="55" cy="45" r="3.8" fill="#fff"/>
  <circle cx="55.5" cy="45" r="1.9" fill="#161616"/>
  <circle cx="56.3" cy="44" r="0.7" fill="#fff"/>
  <path d="M40 58 L46 64 L62 52" stroke="#fff" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>
</svg>
```

- [ ] **Step 3: Render the PNG at 1024×1024**

Use `rsvg-convert` (preferred, available on most NixOS / macOS dev boxes). If absent on the host, run via `nix run nixpkgs#librsvg --`.

```bash
nix run nixpkgs#librsvg -- -w 1024 -h 1024 \
  -o assets/character-on-coal.png \
  assets/character-on-coal.svg
```

- [ ] **Step 4: Verify the PNG**

```bash
file assets/character-on-coal.png
```

Expected: `PNG image data, 1024 x 1024, ...`

```bash
xxd -l 8 assets/character-on-coal.png
```

Expected first eight bytes: `89 50 4e 47 0d 0a 1a 0a` (the PNG signature).

- [ ] **Step 5: Build the lockup SVG (mark + wordmark, glyphs as paths)**

The mark geometry comes from `docs/brand-guidelines.html` line 547–553 (L1 lockup). The wordmark "right agent" must be converted from text to path data. Use `text-to-svg` (Node) — this is the most portable recipe and does not require Inter to be installed system-wide.

Generate the wordmark paths into a temporary file, then hand-assemble the final SVG.

```bash
# Workspace for the one-off generation. Not committed.
mkdir -p /tmp/right-wordmark && cd /tmp/right-wordmark
npm init -y >/dev/null
npm i text-to-svg >/dev/null
curl -sL 'https://github.com/rsms/inter/raw/v4.0/docs/font-files/Inter-Bold.ttf' \
  -o Inter-Bold.ttf

# Emit raw <path d="..."/> for "right" and " agent", plus their advance widths.
node > wordmark.json <<'EOF'
const TTS = require('text-to-svg');
const tts = TTS.loadSync('./Inter-Bold.ttf');
const opts = { fontSize: 36, anchor: 'left top', letterSpacing: -0.025, kerning: true };
const right_d = tts.getD('right', opts);
const right_w = tts.getMetrics('right', opts).width;
const agent_d = tts.getD(' agent', opts);
const agent_w = tts.getMetrics(' agent', opts).width;
const total_h = tts.getMetrics('right agent', opts).height;
process.stdout.write(JSON.stringify({ right_d, right_w, agent_d, total_h }));
EOF

cat wordmark.json   # eyeball: right_d should be a long "M…Z M…Z" path string
```

Then hand-write `assets/lockup-horizontal.svg` using the values from `wordmark.json`. Replace `RIGHT_D`, `RIGHT_W`, `AGENT_D`, `TOTAL_H` with the JSON values:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 280 80" width="280" height="80" role="img" aria-label="right agent">
  <!-- L1 mark, scaled 0.6x from 100x100 to 60x60, vertically centered in an 80px box -->
  <g transform="translate(0,10) scale(0.6)">
    <path d="M14 42 L48 22 Q56 18 62 22 L84 36" stroke="#E8632A" stroke-width="8.5" stroke-linecap="round" fill="none"/>
    <path d="M14 58 L48 78 Q56 82 62 78 L84 64" stroke="#E8632A" stroke-width="8.5" stroke-linecap="round" fill="none"/>
    <rect x="8" y="38" width="11" height="24" rx="5.5" fill="#E8632A"/>
    <path d="M38 50 L46 58 L64 40" stroke="#fff" stroke-width="7" stroke-linecap="round" stroke-linejoin="round" fill="none"/>
  </g>
  <!-- wordmark "right" + " agent" — paths from text-to-svg, vertically centered -->
  <g transform="translate(80, 24)">
    <path d="RIGHT_D" fill="#E8632A"/>
    <g transform="translate(RIGHT_W, 0)"><path d="AGENT_D" fill="#666666"/></g>
  </g>
</svg>
```

If the assembled wordmark overruns the `viewBox="0 0 280 80"`, widen the viewBox/width to `RIGHT_W + AGENT_W + 80` rounded up.

- [ ] **Step 6: Visual sanity-check**

Open both SVGs in a browser:

```bash
open assets/character-on-coal.svg
open assets/lockup-horizontal.svg
```

Expected: orange claw + check on a coal circle for the first; orange "right" + dim "agent" wordmark next to the same claw mark for the second. No system font fallback artifacts (the wordmark glyph shapes match Inter Bold).

- [ ] **Step 7: Commit**

```bash
git add assets/lockup-horizontal.svg assets/character-on-coal.svg assets/character-on-coal.png
git commit -m "feat(assets): add lockup-horizontal SVG and character-on-coal PNG"
```

---

### Task 2: Update README.md to use the lockup

**Files:**
- Modify: `README.md` (lines 1–2)

The current top of the README is:

```markdown
# Right Agent

<p align="center">
  <a href="LICENSE">…
```

Replace the H1 with a centered image block. Per the design (Q2.A) the H1 is dropped entirely.

- [ ] **Step 1: Edit `README.md`**

Replace exactly the first line (`# Right Agent`) with:

```html
<p align="center">
  <img src="assets/lockup-horizontal.svg" alt="Right Agent" width="320">
</p>
```

The blank line after must be preserved so the badges paragraph stays separate.

- [ ] **Step 2: Verify**

```bash
head -5 README.md
```

Expected: the `<p align="center"><img …></p>` block on lines 1–3, blank line on line 4, badges `<p align="center">` opening on line 5. **No** `# Right Agent` heading anywhere.

```bash
grep -n '^# Right Agent' README.md
```

Expected: empty output (no match).

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs(readme): replace H1 with brand-guide lockup image"
```

---

### Task 3: Add `bootstrap_photo` module — TDD

**Files:**
- Create: `crates/bot/src/telegram/bootstrap_photo.rs`
- Modify: `crates/bot/src/telegram/mod.rs` (add `pub(crate) mod bootstrap_photo;`)

This module owns:
- The compile-time-embedded PNG bytes.
- The pure predicate `should_send(bootstrap_mode, first_turn_in_chat) -> bool`.
- An async helper `send_if_needed(...)` that calls `bot.send_photo(...)` when the predicate is true and logs a warn on error. (We test the predicate, not `send_if_needed`, since the latter is straight teloxide.)

- [ ] **Step 1: Write the failing tests first**

Create `crates/bot/src/telegram/bootstrap_photo.rs` with **only** the test module (no implementation yet):

```rust
//! Bootstrap welcome photo — embedded asset + send-gating predicate.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicate_only_true_when_both_flags_true() {
        assert!(!should_send(false, false));
        assert!(!should_send(false, true));
        assert!(!should_send(true, false));
        assert!(should_send(true, true));
    }

    #[test]
    fn welcome_png_starts_with_png_magic() {
        // PNG signature: 89 50 4E 47 0D 0A 1A 0A
        assert!(WELCOME_PNG.len() > 8, "PNG asset is empty or truncated");
        assert_eq!(
            &WELCOME_PNG[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "PNG magic bytes mismatch — asset is not a PNG"
        );
    }
}
```

- [ ] **Step 2: Wire the module into `mod.rs`**

Edit `crates/bot/src/telegram/mod.rs`. Find the existing `pub mod attachments;` line (around line 2). Add immediately after it:

```rust
pub(crate) mod bootstrap_photo;
```

- [ ] **Step 3: Run the tests to confirm they fail**

```bash
cargo test -p right-bot --lib telegram::bootstrap_photo
```

Expected: compile error — `should_send`, `WELCOME_PNG` not found.

- [ ] **Step 4: Implement the constant and predicate**

Replace the contents of `crates/bot/src/telegram/bootstrap_photo.rs` with:

```rust
//! Bootstrap welcome photo — embedded asset + send-gating predicate.
//!
//! The PNG is embedded at compile time so the bot has no runtime filesystem
//! dependency on the asset. Anchoring on `CARGO_MANIFEST_DIR` keeps the path
//! correct regardless of which file inside the crate references this module.

use teloxide::prelude::*;
use teloxide::types::{InputFile, MessageId, ThreadId};

pub(crate) const WELCOME_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/character-on-coal.png"
));

/// Pure predicate. The welcome photo goes out on the *first* CC invocation in
/// a chat **only when** that invocation is happening in bootstrap mode.
pub(crate) fn should_send(bootstrap_mode: bool, first_turn_in_chat: bool) -> bool {
    bootstrap_mode && first_turn_in_chat
}

/// Send the welcome photo to `chat_id` (and the given thread, if any).
///
/// Fire-and-forget: errors are logged at WARN and do not propagate. The text
/// reply is the contract; the photo is presentation.
pub(crate) async fn send_if_needed(
    bot: &teloxide::Bot,
    chat_id: ChatId,
    eff_thread_id: i64,
    bootstrap_mode: bool,
    first_turn_in_chat: bool,
) {
    if !should_send(bootstrap_mode, first_turn_in_chat) {
        return;
    }
    let file = InputFile::memory(WELCOME_PNG.to_vec()).file_name("welcome.png");
    let mut req = bot.send_photo(chat_id, file);
    if eff_thread_id != 0 {
        req = req.message_thread_id(ThreadId(MessageId(eff_thread_id as i32)));
    }
    if let Err(e) = req.await {
        tracing::warn!(?chat_id, eff_thread_id, "bootstrap welcome photo failed: {:#}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicate_only_true_when_both_flags_true() {
        assert!(!should_send(false, false));
        assert!(!should_send(false, true));
        assert!(!should_send(true, false));
        assert!(should_send(true, true));
    }

    #[test]
    fn welcome_png_starts_with_png_magic() {
        assert!(WELCOME_PNG.len() > 8, "PNG asset is empty or truncated");
        assert_eq!(
            &WELCOME_PNG[..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            "PNG magic bytes mismatch — asset is not a PNG"
        );
    }
}
```

- [ ] **Step 5: Run the tests, confirm they pass**

```bash
cargo test -p right-bot --lib telegram::bootstrap_photo
```

Expected: `2 passed; 0 failed`.

- [ ] **Step 6: Confirm no clippy/build warnings**

```bash
cargo clippy -p right-bot --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/bootstrap_photo.rs crates/bot/src/telegram/mod.rs
git commit -m "feat(bot): add bootstrap_photo module with predicate and PNG asset"
```

---

### Task 4: Plumb `is_first_call` out of `invoke_cc`

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` — `invoke_cc` signature, `invoke_cc` return sites, the single call site at line ~555.

Today `invoke_cc` returns `Result<(Option<ReplyOutput>, String), InvokeCcFailure>`. The String is `session_uuid`. We need to also expose `is_first_call` (already a local at line 1134) so the outer worker loop can drive the bootstrap-photo decision.

Wrap the success payload in a struct rather than growing the tuple to three.

- [ ] **Step 1: Add the struct and update the signature**

In `crates/bot/src/telegram/worker.rs`, just above `pub(crate) enum InvokeCcFailure {` (around line 1091), add:

```rust
/// Successful payload returned by [`invoke_cc`].
pub(crate) struct CcReply {
    /// Parsed agent reply, or `None` when CC produced an empty/no-reply result.
    pub output: Option<ReplyOutput>,
    /// CC session UUID for this invocation (new or resumed).
    pub session_uuid: String,
    /// `true` if this invocation created a brand-new CC session
    /// (i.e. the worker's first turn in this chat/thread).
    pub is_first_call: bool,
}
```

Change the function signature at line 1121–1128:

```rust
async fn invoke_cc(
    input: &str,
    first_text: Option<&str>,
    chat_id: i64,
    eff_thread_id: i64,
    is_group: bool,
    ctx: &WorkerContext,
) -> Result<CcReply, InvokeCcFailure> {
```

- [ ] **Step 2: Update the success return at the bottom of `invoke_cc`**

At line 1849 inside `invoke_cc`, replace:

```rust
            Ok((Some(reply_output), session_uuid))
```

with:

```rust
            Ok(CcReply {
                output: Some(reply_output),
                session_uuid,
                is_first_call,
            })
```

`is_first_call` is already in scope at this point (defined at line 1134 — `let (cmd_args, is_first_call, session_uuid) = ...`).

- [ ] **Step 3: Update the call site in the worker loop**

At line 553–566, replace:

```rust
            let (reply_result, session_uuid) =
                match invoke_cc(&input, first_text, chat_id, eff_thread_id, is_group, &ctx).await {
                    Ok((output, uuid)) => (Ok(output), uuid),
                    Err(failure) => {
                        let uuid = match &failure {
                            InvokeCcFailure::Reflectable { session_uuid, .. } => {
                                session_uuid.clone()
                            }
                            InvokeCcFailure::NonReflectable { .. } => String::new(),
                        };
                        (Err(failure), uuid)
                    }
                };
```

with:

```rust
            let (reply_result, session_uuid, is_first_call) =
                match invoke_cc(&input, first_text, chat_id, eff_thread_id, is_group, &ctx).await {
                    Ok(CcReply { output, session_uuid, is_first_call }) => {
                        (Ok(output), session_uuid, is_first_call)
                    }
                    Err(failure) => {
                        let uuid = match &failure {
                            InvokeCcFailure::Reflectable { session_uuid, .. } => {
                                session_uuid.clone()
                            }
                            InvokeCcFailure::NonReflectable { .. } => String::new(),
                        };
                        (Err(failure), uuid, false)
                    }
                };
```

The `false` on the failure branch is correct: a failed invocation does not warrant a welcome photo regardless of session newness.

- [ ] **Step 4: Build and run all bot tests to confirm no regressions**

```bash
cargo build -p right-bot
cargo test -p right-bot
```

Expected: clean build, all tests pass. (`is_first_call` is now an unused local on the worker side until Task 5; rustc will accept it because it is bound by destructuring.)

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "refactor(bot): expose is_first_call from invoke_cc via CcReply struct"
```

---

### Task 5: Send the bootstrap photo from the worker reply branch

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` — success branch around line 644.

Hook the photo send in the `Ok(Some(output)) =>` arm, *before* the text send loop. `bootstrap_mode` is already in scope (line 571). `is_first_call` is now in scope from Task 4. `tg_chat_id` and `eff_thread_id` are in scope.

- [ ] **Step 1: Insert the photo send before the text send loop**

Locate the `Ok(Some(output)) => {` arm in the worker reply branch (around line 645). Immediately after the `let reply_to = ...;` block ends (just before `if let Some(content) = output.content {` at line 656), insert:

```rust
                    // Bootstrap welcome photo — first agent reply only, in
                    // bootstrap mode only. Fire-and-forget: errors logged at
                    // WARN, never block the text reply.
                    super::bootstrap_photo::send_if_needed(
                        &ctx.bot,
                        tg_chat_id,
                        eff_thread_id,
                        bootstrap_mode,
                        is_first_call,
                    )
                    .await;
```

The result: photo lands first, text reply lands second. Order in chat: claw-with-eyes → "Hi, what should I call you?".

- [ ] **Step 2: Build**

```bash
cargo build -p right-bot
```

Expected: clean build, no `unused_variables` warnings on `is_first_call`.

- [ ] **Step 3: Clippy + tests for the bot crate**

```bash
cargo clippy -p right-bot --all-targets -- -D warnings
cargo test -p right-bot
```

Expected: no warnings, all tests pass.

- [ ] **Step 4: Whole-workspace build (sanity)**

```bash
cargo build --workspace
```

Expected: clean.

- [ ] **Step 5: Manual verification (only if a dev sandbox is available)**

Skip if you do not have a Telegram-enabled dev agent ready. Otherwise:

1. Pick a fresh agent name: `right agent init smoke-logo` (this creates a fresh `BOOTSTRAP.md`).
2. `right up`.
3. From Telegram, send any message to the new bot.
4. Expected: the bot replies with two Telegram messages — first the welcome photo (claw with eyes on coal), then the bootstrap greeting text.
5. Send a follow-up message to the bot. Expected: text reply only, no photo. (Still bootstrap, but no longer first call.)
6. Complete bootstrap (answer name/vibe/etc.). Send another message after `BOOTSTRAP.md` is removed. Expected: text reply only, no photo.
7. Tear down: `right down`, then `right agent rm smoke-logo`.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(bot): send bootstrap welcome photo with first agent reply"
```

---

## Self-review against spec

- **Spec → README change:** Task 2 implements the H1 → centered `<img>` swap and verifies via `grep`/`head`.
- **Spec → asset deliverables (`lockup-horizontal.svg`, `character-on-coal.svg`, `character-on-coal.png`):** Task 1 creates all three with concrete recipes; the wordmark-as-paths constraint is honored via `text-to-svg`.
- **Spec → trigger predicate (`bootstrap_mode && first_turn_in_chat`):** Task 3 implements as `should_send`, table-tested.
- **Spec → asset embedding via `include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), …))`:** Task 3.
- **Spec → send order (photo before text):** Task 5, Step 1 inserts the call before the text loop.
- **Spec → thread/topic correctness:** `send_if_needed` mirrors the existing thread-id pattern from `worker.rs:669–674`.
- **Spec → failure handling (warn + continue):** `send_if_needed` wraps `req.await` in `if let Err(e) = … tracing::warn!`.
- **Spec → no agent prompt changes:** Confirmed — no edits to `BOOTSTRAP.md`, `prompt.rs`, schemas, or `invocation.rs`.
- **Spec → tests (PNG magic + predicate truth table):** Task 3, Steps 1 and 4.
- **Spec → only first bootstrap turn (Q3.A):** `is_first_call` collapses to false on the second invocation in the same chat (because `invoke_cc` then takes the `--resume` branch at line 1140), so subsequent turns silently no-op.

No placeholders detected. Type names consistent across tasks (`CcReply`, `WELCOME_PNG`, `should_send`, `send_if_needed`).
