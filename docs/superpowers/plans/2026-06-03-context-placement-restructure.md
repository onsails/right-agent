# Foreground Context Placement Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the per-turn `claude -p` system prompt byte-stable so the Anthropic prompt cache is read instead of re-created every turn, by moving volatile Hindsight recall + status markers out of the system prompt and onto the current user message.

**Architecture:** The composite `--system-prompt-file` keeps only stable content (base prompt, operating instructions, identity files, file-mode `MEMORY.md`, MCP instructions, and a new per-session chat-context block). All volatile per-turn content (Hindsight recall, an edge-triggered `<memory-status>` marker, and the one-shot repair-notice) is prepended to the user message piped on stdin, before the `messages:` YAML. The shared `composite-memory.md` file and its sandbox upload are removed; the system-prompt file becomes per-session to carry the chat-context block without a cross-chat race.

**Tech Stack:** Rust (edition 2024), tokio, `right-prompt-safety` (ironclaw wrap), `right-db` (Turso), `dashmap`. Crates touched: `right-bot` (`crates/bot`). Targeted tests: `cargo test -p right-bot`. Spec: `docs/superpowers/specs/2026-06-03-context-placement-restructure-design.md`.

**Conventions reminder:** FAIL FAST (propagate errors with `?`/`{:#}`, never swallow). Prompt-tier brevity for any agent-facing string. After significant changes, the final `devenv shell -- cargo test --workspace` is mandatory.

**Scope note — topic name is best-effort.** `forum_topics` only tracks topics the *agent itself* created/edited (`crates/right-db/src/forum_topics.rs:1-5`); human-created topics are not in it. The chat-context block therefore always emits `topic_id` and emits `topic` (name) only when found in `forum_topics`. Capturing names for human-created topics is out of scope.

---

## File Structure

- `crates/bot/src/cc/prompt.rs` — prompt assembly. Gains `ChatContextInput`/`ChatContextKind`, `format_chat_context_block`, `build_volatile_prefix` (replaces `format_composite_memory`). `MemoryMode::Hindsight` becomes a unit variant. `build_prompt_assembly_script` gains a `chat_context` param and stops emitting Hindsight memory into the system prompt. Removes `deploy_composite_memory`, `remove_composite_memory`, `SandboxRef`, `DeployError`.
- `crates/bot/src/cc/prompt_tests.rs` — update `test_script` helper for the new param; add coverage.
- `crates/bot/src/telegram/worker.rs` — `edge_memory_marker` + `MEMORY_RECOVERED_MARKER`; `WorkerContext` gains `memory_status_last`; `invoke_cc` builds the volatile prefix and chat-context block, prepends the prefix to stdin, uses a per-session prompt-file path, drops the deploy/remove/bg-marker path, and stops appending the repair-notice to the base prompt. Removes `build_bg_marker_for_chat` and `append_repair_notice_to_system_prompt`.
- `crates/bot/src/telegram/attachments.rs` — `format_cc_input` becomes sequence-only (DM drops `author`+`chat`; group drops `chat`, keeps `author`).
- `PROMPT_SYSTEM.md`, `ARCHITECTURE.md` — doc sync.

---

## Task 1: `format_chat_context_block` (per-session chat context for the system prompt)

**Files:**
- Modify: `crates/bot/src/cc/prompt.rs` (add types + function near the top, after `PromptSection`)
- Test: `crates/bot/src/cc/prompt_tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/bot/src/cc/prompt_tests.rs`:

```rust
#[test]
fn chat_context_block_dm_has_partner_no_group_fields() {
    let block = format_chat_context_block(&ChatContextInput {
        chat_id: 456,
        kind: ChatContextKind::Dm {
            name: "Alice",
            username: Some("alice"),
            user_id: Some(789),
        },
    });
    assert!(block.contains("## Current Conversation"));
    assert!(block.contains("chat_id: 456"));
    assert!(block.contains("kind: dm"));
    assert!(block.contains("Alice"));
    assert!(block.contains("@alice"));
    assert!(block.contains("789"));
    assert!(!block.contains("topic"));
}

#[test]
fn chat_context_block_group_has_title_topic_name() {
    let block = format_chat_context_block(&ChatContextInput {
        chat_id: -100123,
        kind: ChatContextKind::Group {
            title: Some("Team"),
            topic_id: Some(7),
            topic_name: Some("Planning"),
        },
    });
    assert!(block.contains("kind: group"));
    assert!(block.contains("chat_id: -100123"));
    assert!(block.contains("Team"));
    assert!(block.contains("topic_id: 7"));
    assert!(block.contains("Planning"));
}

#[test]
fn chat_context_block_group_omits_absent_topic_name() {
    let block = format_chat_context_block(&ChatContextInput {
        chat_id: -100123,
        kind: ChatContextKind::Group {
            title: None,
            topic_id: Some(7),
            topic_name: None,
        },
    });
    assert!(block.contains("topic_id: 7"));
    assert!(!block.contains("topic:"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot chat_context_block`
Expected: FAIL to compile — `format_chat_context_block`, `ChatContextInput`, `ChatContextKind` not found.

- [ ] **Step 3: Implement the types and function**

In `crates/bot/src/cc/prompt.rs`, after the `PromptSection` struct (around line 44), add:

```rust
/// Stable per-session chat identity emitted into the system prompt. Replaces
/// the constant `author`/`chat` YAML that was repeated on every message.
pub(crate) struct ChatContextInput<'a> {
    pub chat_id: i64,
    pub kind: ChatContextKind<'a>,
}

pub(crate) enum ChatContextKind<'a> {
    Dm {
        name: &'a str,
        username: Option<&'a str>,
        user_id: Option<i64>,
    },
    Group {
        title: Option<&'a str>,
        topic_id: Option<i64>,
        topic_name: Option<&'a str>,
    },
}

/// Render the chat-context block. Pure; stable input → byte-identical output
/// so it stays inside the cached system-prompt prefix.
pub(crate) fn format_chat_context_block(input: &ChatContextInput) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(128);
    out.push_str("## Current Conversation\n");
    let _ = writeln!(out, "chat_id: {}", input.chat_id);
    match &input.kind {
        ChatContextKind::Dm {
            name,
            username,
            user_id,
        } => {
            out.push_str("kind: dm\n");
            let _ = write!(out, "user: {name}");
            if let Some(u) = username {
                let _ = write!(out, " (@{u}");
                if let Some(id) = user_id {
                    let _ = write!(out, ", id {id}");
                }
                out.push(')');
            } else if let Some(id) = user_id {
                let _ = write!(out, " (id {id})");
            }
            out.push('\n');
        }
        ChatContextKind::Group {
            title,
            topic_id,
            topic_name,
        } => {
            out.push_str("kind: group\n");
            if let Some(t) = title {
                let _ = writeln!(out, "title: {t}");
            }
            if let Some(tid) = topic_id {
                let _ = writeln!(out, "topic_id: {tid}");
            }
            if let Some(tn) = topic_name {
                let _ = writeln!(out, "topic: {tn}");
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot chat_context_block`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cc/prompt.rs crates/bot/src/cc/prompt_tests.rs
git commit -m "feat(prompt): add per-session chat-context block builder"
```

---

## Task 2: `build_volatile_prefix` (replaces `format_composite_memory`)

This builds the volatile block prepended to the user message: ironclaw-wrapped recall under our untrusted-content label, plus bot-trusted markers. Returns `None` when there is nothing to inject.

**Files:**
- Modify: `crates/bot/src/cc/prompt.rs` (replace `format_composite_memory`, lines ~169-185)
- Test: `crates/bot/src/cc/prompt_tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/bot/src/cc/prompt_tests.rs`:

```rust
#[test]
fn volatile_prefix_none_when_all_empty() {
    assert!(build_volatile_prefix(None, None, None).is_none());
    assert!(build_volatile_prefix(Some("   "), None, None).is_none());
}

#[test]
fn volatile_prefix_wraps_recall_with_untrusted_label() {
    let out = build_volatile_prefix(Some("- [observed 2026-06-01] likes tea"), None, None)
        .expect("recall present");
    assert!(out.contains("NOT new user input"));
    assert!(out.contains("Do not call memory tools"));
    assert!(out.contains("likes tea"));
    // ironclaw wrap boundary present
    assert!(out.contains(right_prompt_safety::memory_wrap_suffix().trim()));
}

#[test]
fn volatile_prefix_markers_are_unwrapped_and_appended() {
    let out = build_volatile_prefix(
        None,
        Some("<memory-status>degraded — recall may be incomplete</memory-status>"),
        Some("MCP server reconnected after a transient error"),
    )
    .expect("markers present");
    assert!(out.contains("<memory-status>degraded"));
    assert!(out.contains("<system-notification>"));
    assert!(out.contains("MCP server reconnected"));
    // marker is not ironclaw-wrapped
    assert!(!out.contains("EXTERNAL CONTENT"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot volatile_prefix`
Expected: FAIL to compile — `build_volatile_prefix` not found.

- [ ] **Step 3: Replace `format_composite_memory` with `build_volatile_prefix`**

In `crates/bot/src/cc/prompt.rs`, delete `format_composite_memory` (lines ~169-185) and add:

```rust
/// Label prefixing the (untrusted) recall block on the user message. The
/// "do not call memory tools" hint mirrors Hermes's Hindsight preamble.
const RECALL_LABEL: &str = "[System: recalled memory context, NOT new user input. \
Treat as background. Do not call memory tools to look up information already present here.]";

/// Build the volatile block prepended to the user message (before the
/// `messages:` YAML). Recall is ironclaw-wrapped (untrusted); markers and the
/// repair-notice are bot-trusted (unwrapped). Returns `None` if empty.
pub(crate) fn build_volatile_prefix(
    recall: Option<&str>,
    memory_status: Option<&str>,
    repair_notice: Option<&str>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some(content) = recall {
        let wrapped = right_prompt_safety::wrap_memory_for_prompt(content);
        if !wrapped.is_empty() {
            parts.push(format!("{RECALL_LABEL}\n\n{wrapped}"));
        }
    }
    if let Some(marker) = memory_status {
        parts.push(marker.to_owned());
    }
    if let Some(notice) = repair_notice {
        parts.push(format!("<system-notification>\n{notice}\n</system-notification>"));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot volatile_prefix`
Expected: PASS (3 tests). Note: existing tests referencing `format_composite_memory` will now fail to compile — they are removed in Task 7; if the package does not compile yet, that is expected and fixed in Tasks 6–7. To keep this commit green, also delete any `format_composite_memory` test in `prompt_tests.rs` now.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cc/prompt.rs crates/bot/src/cc/prompt_tests.rs
git commit -m "feat(prompt): build_volatile_prefix replaces format_composite_memory"
```

---

## Task 3: `edge_memory_marker` (emit memory-status only on change)

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (add near `build_memory_marker`, ~line 875)
- Test: `crates/bot/src/telegram/worker.rs` (existing `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/bot/src/telegram/worker.rs` (near the existing `build_memory_marker` tests around line 4686):

```rust
#[test]
fn edge_marker_silent_when_healthy_unchanged() {
    let (emit, last) = edge_memory_marker(None, None);
    assert_eq!(emit, None);
    assert_eq!(last, None);
}

#[test]
fn edge_marker_emits_on_entering_degraded() {
    let (emit, last) = edge_memory_marker(None, Some("<memory-status>degraded</memory-status>"));
    assert_eq!(emit.as_deref(), Some("<memory-status>degraded</memory-status>"));
    assert_eq!(last.as_deref(), Some("<memory-status>degraded</memory-status>"));
}

#[test]
fn edge_marker_silent_while_degraded_unchanged() {
    let (emit, last) = edge_memory_marker(
        Some("<memory-status>degraded</memory-status>"),
        Some("<memory-status>degraded</memory-status>"),
    );
    assert_eq!(emit, None);
    assert_eq!(last.as_deref(), Some("<memory-status>degraded</memory-status>"));
}

#[test]
fn edge_marker_emits_on_degradation_degree_change() {
    let (emit, _) = edge_memory_marker(
        Some("<memory-status>degraded</memory-status>"),
        Some("<memory-status>unavailable</memory-status>"),
    );
    assert_eq!(emit.as_deref(), Some("<memory-status>unavailable</memory-status>"));
}

#[test]
fn edge_marker_emits_recovered_once_then_silent() {
    let (emit, last) =
        edge_memory_marker(Some("<memory-status>degraded</memory-status>"), None);
    assert_eq!(emit.as_deref(), Some(MEMORY_RECOVERED_MARKER));
    assert_eq!(last, None);
    // next healthy turn: prev now None → silent
    let (emit2, _) = edge_memory_marker(None, None);
    assert_eq!(emit2, None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot edge_marker`
Expected: FAIL to compile — `edge_memory_marker`, `MEMORY_RECOVERED_MARKER` not found.

- [ ] **Step 3: Implement**

In `crates/bot/src/telegram/worker.rs`, after `build_memory_marker` (after line 875), add:

```rust
/// Emitted once when memory transitions back to healthy from any non-healthy
/// state. `build_memory_marker` returns `None` for healthy, so recovery needs
/// its own marker.
const MEMORY_RECOVERED_MARKER: &str =
    "<memory-status>recovered — memory provider is healthy again</memory-status>";

/// Edge-trigger the memory-status marker against the value last emitted for
/// this session. Returns `(marker_to_emit, new_last_emitted)`.
///
/// - unchanged → emit nothing;
/// - changed to a non-healthy state → emit it;
/// - changed to healthy (recovery) → emit the recovered marker once.
///
/// `new_last_emitted` tracks the underlying status (`cur`), not the recovered
/// text, so the next healthy turn stays silent.
fn edge_memory_marker(
    prev: Option<&str>,
    cur: Option<&str>,
) -> (Option<String>, Option<String>) {
    if prev == cur {
        return (None, cur.map(str::to_owned));
    }
    match cur {
        Some(m) => (Some(m.to_owned()), Some(m.to_owned())),
        None => (Some(MEMORY_RECOVERED_MARKER.to_owned()), None),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot edge_marker`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): edge-triggered memory-status marker"
```

---

## Task 4: `format_cc_input` sequence-only (drop constant author/chat)

DM: omit `author` and `chat` per message. Group: omit `chat`, keep `author`. The omitted identity now lives in the system-prompt chat-context block (Task 1).

**Files:**
- Modify: `crates/bot/src/telegram/attachments.rs:458-496` (author + chat blocks)
- Test: `crates/bot/src/telegram/attachments.rs` (existing test module)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)]` module in `crates/bot/src/telegram/attachments.rs`. Use the existing test helpers/struct construction style already in that module (see `format_cc_input_*` tests around line 2166+ for the `InputMessage` literal pattern):

```rust
#[tokio::test]
async fn format_cc_input_dm_omits_author_and_chat() {
    let m = InputMessage {
        message_id: 1,
        text: Some("hi".into()),
        timestamp: Utc::now(),
        attachments: vec![],
        author: MessageAuthor {
            name: "Alice".into(),
            username: Some("alice".into()),
            user_id: Some(789),
        },
        forward_info: None,
        reply_to_id: None,
        quoted_text: None,
        chat: ChatContext::Private { id: 789 },
        reply_to_body: None,
    };
    let yaml = format_cc_input(&[m]).unwrap();
    assert!(yaml.contains("text: \"hi\""));
    assert!(!yaml.contains("author:"), "DM must omit author block");
    assert!(!yaml.contains("chat:"), "DM must omit chat block");
}

#[tokio::test]
async fn format_cc_input_group_keeps_author_omits_chat() {
    let m = InputMessage {
        message_id: 2,
        text: Some("yo".into()),
        timestamp: Utc::now(),
        attachments: vec![],
        author: MessageAuthor {
            name: "Bob".into(),
            username: None,
            user_id: Some(42),
        },
        forward_info: None,
        reply_to_id: None,
        quoted_text: None,
        chat: ChatContext::Group {
            id: -100,
            title: Some("Team".into()),
            topic_id: Some(7),
        },
        reply_to_body: None,
    };
    let yaml = format_cc_input(&[m]).unwrap();
    assert!(yaml.contains("author:"), "group keeps per-message author");
    assert!(yaml.contains("Bob"));
    assert!(!yaml.contains("chat:"), "group omits chat block");
    assert!(!yaml.contains("topic_id:"), "topic now lives in system prompt");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot format_cc_input_dm_omits format_cc_input_group_keeps`
Expected: FAIL — current output still contains `author:`/`chat:`.

- [ ] **Step 3: Implement sequence-only emission**

In `crates/bot/src/telegram/attachments.rs`, replace the author block (lines 458-472) and chat block (lines 474-496) with author emission gated on group and chat emission removed:

```rust
        // Author block: omitted in DMs (constant — lives in the system-prompt
        // chat-context block). Kept per-message in groups (multi-user).
        if let ChatContext::Group { .. } = m.chat {
            out.push_str("    author:\n");
            writeln!(
                out,
                "      name: \"{}\"",
                yaml_escape_string(&m.author.name)
            )
            .expect("infallible");
            if let Some(ref username) = m.author.username {
                writeln!(out, "      username: \"{}\"", yaml_escape_string(username))
                    .expect("infallible");
            }
            if let Some(user_id) = m.author.user_id {
                writeln!(out, "      user_id: {user_id}").expect("infallible");
            }
        }

        // Chat identity is omitted entirely — it is constant per session and
        // now lives in the system-prompt chat-context block.
```

(Delete the entire old `// Chat block — always present.` section, lines 474-496.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot format_cc_input`
Expected: PASS for the two new tests. **Existing `format_cc_input_*` tests that assert on `chat:`/`author:` in DMs will need updating** — adjust their assertions to the new sequence-only output (remove `chat:`/DM-`author:` expectations). Run the full `format_cc_input` filter and fix each failing assertion to match the new contract.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/attachments.rs
git commit -m "feat(attachments): sequence-only message YAML (drop constant author/chat)"
```

---

## Task 5: `build_prompt_assembly_script` — chat-context param, drop Hindsight memory, unit `Hindsight`

**Files:**
- Modify: `crates/bot/src/cc/prompt.rs` (`MemoryMode` enum line 4-9; `build_prompt_assembly_script` signature + body line 76-166)
- Modify: `crates/bot/src/cc/prompt_tests.rs` (`test_script` helper)

- [ ] **Step 1: Update the test helper and add a coverage test**

In `crates/bot/src/cc/prompt_tests.rs`, change `test_script` to pass the new `chat_context` argument (None by default) and add a test:

```rust
fn test_script(base: &str, mode: PromptMode, args: &[String], mcp: Option<&str>) -> String {
    build_prompt_assembly_script(
        base,
        mode,
        "/sandbox",
        "/tmp/right-system-prompt.md",
        "/sandbox",
        args,
        mcp,
        None,          // memory_mode
        None,          // chat_context
    )
}

#[tokio::test]
async fn script_hindsight_mode_emits_no_memory_section() {
    let script = build_prompt_assembly_script(
        "Base",
        PromptMode::Normal,
        "/sandbox",
        "/tmp/right-system-prompt.md",
        "/sandbox",
        &["claude".into(), "-p".into()],
        None,
        Some(&MemoryMode::Hindsight),
        Some("## Current Conversation\nchat_id: 1\nkind: dm\n"),
    );
    assert!(!script.contains("composite-memory.md"));
    assert!(script.contains("## Current Conversation"));
}

#[tokio::test]
async fn script_file_mode_still_emits_memory_md() {
    let script = build_prompt_assembly_script(
        "Base",
        PromptMode::Normal,
        "/sandbox",
        "/tmp/right-system-prompt.md",
        "/sandbox",
        &["claude".into(), "-p".into()],
        None,
        Some(&MemoryMode::File),
        None,
    );
    assert!(script.contains("MEMORY.md"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right-bot script_hindsight_mode script_file_mode`
Expected: FAIL to compile — arity mismatch / `MemoryMode::Hindsight` still has a field.

- [ ] **Step 3: Make `Hindsight` a unit variant**

In `crates/bot/src/cc/prompt.rs`, change the enum (lines 4-9):

```rust
pub(crate) enum MemoryMode {
    /// Inject MEMORY.md from agent directory (file mode).
    File,
    /// Hindsight mode — recall is injected on the user message, not here.
    Hindsight,
}
```

- [ ] **Step 4: Add the `chat_context` param, drop the Hindsight memory branch, emit the chat-context section**

In `build_prompt_assembly_script` (line 76), add a parameter after `memory_mode`:

```rust
    memory_mode: Option<&MemoryMode>,
    chat_context: Option<&str>,
) -> String {
```

Replace the Hindsight match arm (lines 152-159) so it emits nothing:

```rust
            Some(MemoryMode::Hindsight) => String::new(),
```

Before the final `format!` (line 164), build the chat-context section:

```rust
    let chat_context_section = match chat_context {
        Some(ctx) if !ctx.trim().is_empty() => {
            let escaped = ctx.replace('\'', "'\\''");
            format!("\nprintf '\\n'\nprintf '%s\\n' '{escaped}'")
        }
        _ => String::new(),
    };
```

Then add `{chat_context_section}` to the assembled body, after `file_sections` and before `mcp_section`:

```rust
    format!(
        "{sandbox_env_prelude}\n{{ printf '{escaped_base}'\n{file_sections}\n{chat_context_section}\n{mcp_section}\n{memory_section}\n}} > {prompt_file}\ncd {workdir} && {claude_cmd} --system-prompt-file {prompt_file}"
    )
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p right-bot script_`
Expected: PASS. (Other call sites in worker.rs still pass the old arity and won't compile yet — fixed in Task 6. If you need a green checkpoint, proceed directly to Task 6 before re-running package-wide tests.)

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cc/prompt.rs crates/bot/src/cc/prompt_tests.rs
git commit -m "feat(prompt): chat-context section; Hindsight recall leaves system prompt"
```

---

## Task 6: Wire `invoke_cc` — volatile prefix on stdin, per-session prompt file, chat-context

This is the integration task. No new unit test (it is end-to-end glue); verified by `cargo build` + the existing worker tests + a manual reproduce. Make the edits precisely.

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs` (`WorkerContext` struct ~268-326; `invoke_cc` signature ~2702-2710; memory block ~2842-2950; assembly call sites ~2983, ~3023; stdin write ~3208; caller ~1449)

- [ ] **Step 1: Add the edge-state field to `WorkerContext`**

In `crates/bot/src/telegram/worker.rs`, in `pub struct WorkerContext` (after `prefetch_cache`, line 317), add:

```rust
    /// Last `<memory-status>` value emitted per session, for edge-triggering.
    /// Absent key = healthy baseline. In-memory only; a restart may re-emit
    /// once (harmless).
    pub memory_status_last: Arc<DashMap<SessionKey, String>>,
```

- [ ] **Step 2: Initialize the field at every `WorkerContext` construction site**

Run: `rg -n 'WorkerContext \{' crates/bot/src` to list construction sites (production + tests). At each, add:

```rust
        memory_status_last: Arc::new(DashMap::new()),
```

- [ ] **Step 3: Extend `invoke_cc` signature with chat + author**

Change `invoke_cc` (line 2702) to accept the triggering message's chat and author for the chat-context block:

```rust
async fn invoke_cc(
    input: &str,
    first_text: Option<&str>,
    chat_id: i64,
    eff_thread_id: i64,
    is_group: bool,
    routed_message_ids: &[i32],
    chat: &super::attachments::ChatContext,
    author: &super::attachments::MessageAuthor,
    ctx: &WorkerContext,
) -> Result<CcReply, InvokeCcFailure> {
```

- [ ] **Step 4: Stop appending the repair-notice to the base prompt**

Replace lines 2837-2840 so `base_prompt` stays clean (the notice is routed to the volatile prefix in Step 6):

```rust
    let base_prompt =
        right_codegen::generate_system_prompt(&ctx.agent_name, &sandbox_mode, &home_dir);
```

(`repair_notice` from line 2832-2836 is kept and used in Step 6.)

- [ ] **Step 5: Rewrite the memory block to build a volatile prefix (drop deploy/remove/bg-marker)**

Replace the whole `let memory_mode = if ctx.hindsight.is_some() { ... } else { Some(File) };` block (lines 2842-2950) with:

```rust
    let session_key: SessionKey = (chat_id, eff_thread_id);
    let mut volatile_prefix: Option<String> = None;
    let memory_mode = if ctx.hindsight.is_some() {
        // Recall (prefetch cache, else blocking) — unchanged logic.
        let cache_key = format!("{}:{}", chat_id, eff_thread_id);
        let cached = if let Some(ref cache) = ctx.prefetch_cache {
            cache.get(&cache_key).await
        } else {
            None
        };
        let recall_content = if let Some(content) = cached {
            Some(content)
        } else if let Some(ref hs) = ctx.hindsight {
            tracing::info!(?chat_id, "prefetch cache miss, blocking recall");
            let truncated_query = truncate_to_chars(input, RECALL_MAX_CHARS);
            let recall_tags_v = recall_tags(chat_id);
            match hs
                .recall(
                    truncated_query,
                    Some(&recall_tags_v),
                    Some("any"),
                    right_memory::resilient::POLICY_BLOCKING_RECALL,
                )
                .await
            {
                Ok(results) if !results.is_empty() => {
                    let content = right_memory::hindsight::render_recall_with_dates(&results);
                    if let Some(ref cache) = ctx.prefetch_cache {
                        cache.put(&cache_key, content.clone()).await;
                    }
                    Some(content)
                }
                Ok(_) => None,
                Err(right_memory::ResilientError::CircuitOpen { .. }) => {
                    tracing::warn!(?chat_id, "blocking recall skipped: circuit open");
                    None
                }
                Err(right_memory::ResilientError::Upstream(e)) => {
                    tracing::warn!(?chat_id, "blocking recall failed: {e:#}");
                    None
                }
            }
        } else {
            None
        };

        // Edge-triggered memory-status.
        let wrapper_status = ctx
            .hindsight
            .as_ref()
            .map(|h| h.status())
            .unwrap_or(right_memory::MemoryStatus::Healthy);
        let client_drops_24h = if let Some(ref h) = ctx.hindsight {
            h.client_drops_24h().await
        } else {
            0
        };
        let cur_marker = build_memory_marker(wrapper_status, client_drops_24h);
        let prev_marker = ctx
            .memory_status_last
            .get(&session_key)
            .map(|r| r.clone());
        let (emit_marker, new_last) =
            edge_memory_marker(prev_marker.as_deref(), cur_marker.as_deref());
        match new_last {
            Some(s) => {
                ctx.memory_status_last.insert(session_key, s);
            }
            None => {
                ctx.memory_status_last.remove(&session_key);
            }
        }

        volatile_prefix = crate::cc::prompt::build_volatile_prefix(
            recall_content.as_deref(),
            emit_marker.as_deref(),
            repair_notice.as_deref(),
        );

        Some(crate::cc::prompt::MemoryMode::Hindsight)
    } else {
        // File mode: repair-notice still needs a home on the message.
        volatile_prefix =
            crate::cc::prompt::build_volatile_prefix(None, None, repair_notice.as_deref());
        Some(crate::cc::prompt::MemoryMode::File)
    };

    // Prepend the volatile block to the user message piped on stdin.
    let effective_input = match volatile_prefix {
        Some(prefix) => format!("{prefix}\n\n{input}"),
        None => input.to_string(),
    };
```

- [ ] **Step 6: Build the chat-context block (with best-effort topic name)**

Immediately after the block above, add:

```rust
    let chat_context_block = {
        use super::attachments::ChatContext as CC;
        let input = match chat {
            CC::Private { id } => crate::cc::prompt::ChatContextInput {
                chat_id: *id,
                kind: crate::cc::prompt::ChatContextKind::Dm {
                    name: &author.name,
                    username: author.username.as_deref(),
                    user_id: author.user_id,
                },
            },
            CC::Group {
                id,
                title,
                topic_id,
            } => {
                // Best-effort topic name from the agent-managed registry.
                let topic_name = match topic_id {
                    Some(tid) => right_db::forum_topics::list(&conn, *id)
                        .await
                        .ok()
                        .and_then(|rows| {
                            rows.into_iter()
                                .find(|r| r.message_thread_id == *tid)
                                .and_then(|r| r.name)
                        }),
                    None => None,
                };
                crate::cc::prompt::ChatContextInput {
                    chat_id: *id,
                    kind: crate::cc::prompt::ChatContextKind::Group {
                        title: title.as_deref(),
                        topic_id: *topic_id,
                        topic_name: topic_name.as_deref(),
                    },
                }
            }
        };
        crate::cc::prompt::format_chat_context_block(&input)
    };
```

(`conn` is the `right_db::Connection` opened at the top of `invoke_cc`, line 2711.)

- [ ] **Step 7: Per-session prompt-file paths + pass chat-context to both assembly call sites**

Sandbox call site (line 2987): change the prompt-file path and add the `chat_context` arg:

```rust
            &format!("/tmp/right-system-prompt-{session_uuid}.md"),
```

and add `Some(chat_context_block.as_str()),` as the final argument to `build_prompt_assembly_script` (after `memory_mode.as_ref()`).

No-sandbox call site (line 3019-3032): change `prompt_path`:

```rust
        let prompt_path = ctx
            .agent_dir
            .join(".claude")
            .join(format!("composite-system-prompt-{session_uuid}.md"));
```

and add `Some(chat_context_block.as_str()),` as the final argument.

- [ ] **Step 8: Pipe `effective_input` to stdin**

At line 3208, change the stdin write from `input` to `effective_input`:

```rust
            result = stdin.write_all(effective_input.as_bytes()) => {
```

- [ ] **Step 9: Update the `invoke_cc` caller**

At the call site (line 1449), the worker loop has `input_messages` (built just above at 1348). Pass the triggering message's chat + author. After the `format_cc_input` guard, capture them:

```rust
            let (trigger_chat, trigger_author) = {
                let m = input_messages
                    .first()
                    .expect("format_cc_input returned Some so input_messages is non-empty");
                (m.chat.clone(), m.author.clone())
            };
```

and pass `&trigger_chat, &trigger_author,` to `invoke_cc(...)` between `&routed_message_ids` and `&ctx`.

- [ ] **Step 10: Build the package**

Run: `devenv shell -- cargo build -p right-bot`
Expected: compiles. Fix any arity/borrow errors surfaced (e.g. remaining old call sites). `repair_notice` must now be unused in `append_repair_notice_to_system_prompt`'s removed path — handled in Task 7.

- [ ] **Step 11: Run worker tests**

Run: `devenv shell -- cargo test -p right-bot`
Expected: PASS (after Task 7 removes the now-dead functions; if `unused function` denies the build under `-D warnings`, do Task 7 before this step).

- [ ] **Step 12: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat(worker): volatile prefix on stdin; per-session system prompt; chat-context block"
```

---

## Task 7: Remove dead code (composite-memory machinery, bg marker, repair-notice helper)

**Files:**
- Modify: `crates/bot/src/cc/prompt.rs` (delete `deploy_composite_memory`, `remove_composite_memory`, `SandboxRef`, `DeployError`, lines ~187-275)
- Modify: `crates/bot/src/telegram/worker.rs` (delete `build_bg_marker_for_chat` ~888-956, `append_repair_notice_to_system_prompt` ~178-186, and `append_system_notification` if now unused ~188-192)

- [ ] **Step 1: Delete the composite-memory writers in `prompt.rs`**

Remove `deploy_composite_memory`, `remove_composite_memory`, `SandboxRef`, and `DeployError` (lines ~187-275). Remove any now-unused imports.

- [ ] **Step 2: Delete `build_bg_marker_for_chat` and the repair-notice helpers in `worker.rs`**

Remove `build_bg_marker_for_chat` (lines ~877-956). Remove `append_repair_notice_to_system_prompt` (lines ~178-186). If `append_system_notification` is no longer referenced anywhere, remove it too (verify with `rg -n append_system_notification crates/bot/src`).

- [ ] **Step 3: Remove dead tests**

Run: `rg -n 'format_composite_memory|deploy_composite_memory|remove_composite_memory|build_bg_marker|append_repair_notice|background-jobs' crates/bot/src` and delete tests referencing removed items.

- [ ] **Step 4: Build + test**

Run: `devenv shell -- cargo test -p right-bot`
Expected: PASS, no dead-code warnings.

- [ ] **Step 5: Clippy**

Run: `devenv shell -- cargo clippy -p right-bot -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/bot/src/cc/prompt.rs crates/bot/src/telegram/worker.rs
git commit -m "refactor(bot): remove composite-memory.md, bg-jobs marker, repair-notice-in-prompt"
```

---

## Task 8: Documentation sync

**Files:**
- Modify: `PROMPT_SYSTEM.md`
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update `PROMPT_SYSTEM.md`**

Update the prompt-assembly description: the composite system prompt is now byte-stable and per-session; it carries identity files + file-mode `MEMORY.md` + a per-session chat-context block; Hindsight recall, the edge-triggered `<memory-status>` marker, and the repair-notice are prepended to the user message (stdin), not the system prompt; `composite-memory.md` and the `<background-jobs>` marker are removed.

- [ ] **Step 2: Update `ARCHITECTURE.md`**

In the **Prompting Architecture** section, note that volatile recall/markers live on the user message and the system prompt is byte-stable per session. Remove/adjust any `composite-memory.md` reference in the Configuration Hierarchy / sandbox notes. Keep it within the 40k budget — prefer a one-line rule and push narration to `docs/architecture/memory.md` if needed.

- [ ] **Step 3: Commit**

```bash
git add PROMPT_SYSTEM.md ARCHITECTURE.md
git commit -m "docs: system prompt byte-stable; recall/markers move to user message"
```

---

## Task 9: Final workspace verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. (Per the flaky-tests note, re-run any cc/invocation pid-race or dashboard warn-count failures in isolation before attributing them to this change.)

- [ ] **Step 2: Workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: success.

- [ ] **Step 3: Manual cache verification (optional but recommended)**

Reproduce a sandbox `claude -p` turn by hand (see AGENTS.md "Reproduce a sandbox `claude` invocation"), run two consecutive foreground turns in the same chat, and confirm in the stream NDJSON `result` usage that `cache_creation_input_tokens` on the second turn dropped to roughly the new message size and `cache_read_input_tokens` covers the full prefix (no `cache_miss_reason: system_changed`).

---

## Self-Review

**Spec coverage:**
- §1 byte-stable per-session system prompt → Tasks 5, 6 (per-session path), 1 (chat-context).
- §1 file-mode MEMORY.md stays → Task 5 (`script_file_mode_still_emits_memory_md`).
- §2 chat-context block (DM/group, topic name best-effort) → Tasks 1, 6.
- §3 volatile block on user message (recall + edge memory-status + repair-notice; empty omitted) → Tasks 2, 3, 6.
- §3 bg-jobs marker removed, no tool added → Task 7.
- §4 sequence-only YAML → Task 4.
- §5 recall accumulation (option A) → inherent (Task 6 prepends to persisted stdin); no extra code.
- §6 per-session prompt-file path → Task 6.
- §7 composite-memory.md removed → Task 7.
- Security (ironclaw wrap kept, no "authoritative" framing) → Task 2 (`RECALL_LABEL`, `wrap_memory_for_prompt`).
- Docs → Task 8. Final test → Task 9.

**Placeholder scan:** none — every code step shows complete code; the only judgement steps (updating existing assertions in Task 4 Step 4, listing construction sites in Task 6 Step 2) specify the exact `rg` command and the change to make.

**Type consistency:** `build_volatile_prefix(Option<&str>,Option<&str>,Option<&str>)->Option<String>`, `edge_memory_marker(Option<&str>,Option<&str>)->(Option<String>,Option<String>)`, `MemoryMode::Hindsight` (unit), `format_chat_context_block(&ChatContextInput)->String`, and the 9-arg `build_prompt_assembly_script` (added `chat_context`) are used consistently across Tasks 2/3/5/6 and the test helper.
