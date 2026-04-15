# Cron Delivery Verbatim Relay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make cron delivery relay `notify.content` verbatim instead of letting Haiku summarize it.

**Architecture:** Add instruction prefix to the stdin payload in `format_cron_yaml()`. System prompt stays unchanged (prompt cache preserved). Document why delivery goes through CC in ARCHITECTURE.md.

**Tech Stack:** Rust, existing `cron_delivery.rs`

**Spec:** `docs/superpowers/specs/2026-04-15-cron-delivery-verbatim-relay.md`

---

### Task 1: Update existing tests and add instruction prefix test

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs:654-683` (existing tests)

- [ ] **Step 1: Update `format_cron_yaml_basic` test to expect instruction prefix**

The function will now prepend instruction text. Update existing assertions and add new ones:

```rust
#[test]
fn format_cron_yaml_basic() {
    let pending = PendingCronResult {
        id: "abc".into(),
        job_name: "health-check".into(),
        notify_json: r#"{"content":"BTC up 2%"}"#.into(),
        summary: "Checked 5 pairs".into(),
        finished_at: "2026-01-01T00:01:00Z".into(),
    };
    let output = format_cron_yaml(&pending, 2);
    // Instruction prefix comes first
    assert!(output.starts_with("You are delivering a cron job result"));
    assert!(output.contains("VERBATIM"));
    assert!(output.contains("attachments are sent separately"));
    // Separator between instruction and YAML
    assert!(output.contains("Here is the YAML report of the cron job:"));
    // YAML content still present
    assert!(output.contains("job: \"health-check\""));
    assert!(output.contains("runs_total: 3"));
    assert!(output.contains("skipped_runs: 2"));
    assert!(output.contains("BTC up 2%"));
    assert!(output.contains("Checked 5 pairs"));
}
```

- [ ] **Step 2: Update `format_cron_yaml_no_skipped` test**

```rust
#[test]
fn format_cron_yaml_no_skipped() {
    let pending = PendingCronResult {
        id: "abc".into(),
        job_name: "job1".into(),
        notify_json: r#"{"content":"hello"}"#.into(),
        summary: "done".into(),
        finished_at: "2026-01-01T00:01:00Z".into(),
    };
    let output = format_cron_yaml(&pending, 0);
    assert!(output.starts_with("You are delivering a cron job result"));
    assert!(output.contains("runs_total: 1"));
    assert!(!output.contains("skipped_runs"));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot format_cron_yaml`
Expected: FAIL — `format_cron_yaml` doesn't produce instruction prefix yet.

### Task 2: Add instruction prefix to `format_cron_yaml()`

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs:107-157` (`format_cron_yaml` function + doc comment)

- [ ] **Step 1: Add instruction constant and update `format_cron_yaml()`**

Add a constant above `format_cron_yaml` and prepend it in the function:

```rust
/// Instruction prefix for the delivery CC session.
///
/// This is approach A: instruction in stdin. If Haiku ignores these instructions
/// (summarizes instead of relaying verbatim), migrate to approach B: add a
/// delivery-specific block to the system prompt via `build_prompt_assembly_script()`.
/// See docs/superpowers/specs/2026-04-15-cron-delivery-verbatim-relay.md.
const DELIVERY_INSTRUCTION: &str = "\
You are delivering a cron job result to the user.
The `content` field below is the FINAL user-facing message — send it VERBATIM in your response.
Do NOT summarize, rephrase, or omit any part of the content.
You MAY prepend a short contextual intro (1 sentence max) if recent conversation was on a different topic, so the message feels natural.
Ignore the attachments field — attachments are sent separately.

Here is the YAML report of the cron job:
";
```

Update `format_cron_yaml()`:

```rust
/// Format a pending cron result for the delivery CC session.
///
/// Output: instruction prefix (verbatim relay directive) + YAML payload.
/// The instruction tells Haiku to forward `notify.content` as-is.
pub fn format_cron_yaml(pending: &PendingCronResult, skipped: u32) -> String {
    let total = skipped + 1;
    let mut output = String::from(DELIVERY_INSTRUCTION);
    output.push_str("\ncron_result:\n");
    // ... rest unchanged
```

Only the first two lines of the function body change: `String::new()` → `String::from(DELIVERY_INSTRUCTION)` and add an extra `\n` before `cron_result:`.

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot format_cron_yaml`
Expected: PASS

- [ ] **Step 3: Run full workspace build**

Run: `cargo build --workspace`
Expected: success

- [ ] **Step 4: Commit**

```bash
git add crates/bot/src/cron_delivery.rs
git commit -m "fix: add verbatim relay instruction to cron delivery stdin"
```

### Task 3: Document delivery rationale in ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md:87` (cron_delivery.rs comment in module map)

- [ ] **Step 1: Update cron_delivery.rs description**

Change line 87 from:
```
├── cron_delivery.rs    # Delivery poll loop: idle detection, dedup, CC session delivery (haiku), cleanup
```
to:
```
├── cron_delivery.rs    # Delivery poll loop: idle detection, dedup, CC session delivery (haiku), cleanup. Resumes main session so cron results land in agent conversation context.
```

- [ ] **Step 2: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: document why cron delivery goes through CC session"
```
