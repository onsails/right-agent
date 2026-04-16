# Cron Memory Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove auto-recall and auto-retain from cron/delivery sessions, fix worker recall truncation to count characters instead of bytes.

**Architecture:** Three changes in the bot crate: (1) strip all Hindsight memory operations from `cron.rs` and `cron_delivery.rs`, removing the `hindsight` and `prefetch_cache` parameters that thread through the entire cron call chain; (2) fix `truncate_to_char_boundary` in `worker.rs` to count characters, not bytes; (3) update ARCHITECTURE.md to match.

**Tech Stack:** Rust, tokio, rightclaw bot crate

---

### Task 1: Fix worker recall truncation — bytes → characters

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:35` (constant)
- Modify: `crates/bot/src/telegram/worker.rs:242-252` (function)
- Modify: `crates/bot/src/telegram/worker.rs:1672-1711` (tests)

- [ ] **Step 1: Update the existing tests to assert character-count semantics**

Replace the test module's truncation tests in `crates/bot/src/telegram/worker.rs` (lines 1672–1711). The new tests assert that `truncate_to_chars` limits by character count, not byte count:

```rust
    #[test]
    fn truncate_to_chars_short_string() {
        assert_eq!(truncate_to_chars("hello", 800), "hello");
    }

    #[test]
    fn truncate_to_chars_exact_limit() {
        let s = "a".repeat(800);
        assert_eq!(truncate_to_chars(&s, 800).chars().count(), 800);
    }

    #[test]
    fn truncate_to_chars_over_limit() {
        let s = "a".repeat(1000);
        assert_eq!(truncate_to_chars(&s, 800).chars().count(), 800);
    }

    #[test]
    fn truncate_to_chars_multibyte() {
        // 'é' is 2 bytes in UTF-8. 1000 chars × 2 bytes = 2000 bytes.
        // Truncation must keep 800 characters (1600 bytes), not 800 bytes (400 chars).
        let s = "é".repeat(1000);
        let truncated = truncate_to_chars(&s, 800);
        assert_eq!(truncated.chars().count(), 800);
        assert_eq!(truncated.len(), 1600); // 800 chars × 2 bytes
    }

    #[test]
    fn truncate_to_chars_emoji() {
        // '🎯' is 4 bytes. 1000 chars × 4 bytes = 4000 bytes.
        // Truncation must keep 800 characters (3200 bytes), not 800 bytes (200 chars).
        let s = "🎯".repeat(1000);
        let truncated = truncate_to_chars(&s, 800);
        assert_eq!(truncated.chars().count(), 800);
        assert_eq!(truncated.len(), 3200); // 800 chars × 4 bytes
    }

    #[test]
    fn truncate_to_chars_empty() {
        assert_eq!(truncate_to_chars("", 800), "");
    }

    #[test]
    fn truncate_to_chars_cyrillic() {
        // Cyrillic chars are 2 bytes each. 500 chars = 1000 bytes.
        // Should keep all 500 chars (under 800 limit).
        let s = "я".repeat(500);
        let truncated = truncate_to_chars(&s, 800);
        assert_eq!(truncated.chars().count(), 500);
        assert_eq!(truncated, s);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p rightclaw-bot truncate_to_chars 2>&1 | tail -20`
Expected: FAIL — `truncate_to_chars` does not exist yet

- [ ] **Step 3: Replace the function and constant**

In `crates/bot/src/telegram/worker.rs`, replace the constant at line 35:

```rust
// Old:
const RECALL_MAX_INPUT_CHARS: usize = 800;

// New:
const RECALL_MAX_CHARS: usize = 800;
```

Replace the function at lines 242–252:

```rust
// Old:
/// Truncate a string to at most `max` bytes on a valid UTF-8 char boundary.
fn truncate_to_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// New:
/// Truncate a string to at most `max_chars` characters (not bytes).
///
/// Hindsight recall API rejects queries over 500 tokens. At ~1 token per
/// 1.5 chars, 800 chars stays safely under that limit.
fn truncate_to_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}
```

Update the two call sites that reference the old names. At line 573:

```rust
// Old:
let recall_query = truncate_to_char_boundary(&input, RECALL_MAX_INPUT_CHARS).to_owned();

// New:
let recall_query = truncate_to_chars(&input, RECALL_MAX_CHARS).to_owned();
```

At line 863:

```rust
// Old:
let truncated_query = truncate_to_char_boundary(input, RECALL_MAX_INPUT_CHARS);

// New:
let truncated_query = truncate_to_chars(input, RECALL_MAX_CHARS);
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `devenv shell -- cargo test -p rightclaw-bot truncate_to_chars 2>&1 | tail -20`
Expected: all 7 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "fix(worker): truncate recall queries by character count, not bytes

Hindsight recall API rejects queries over 500 tokens. The old
truncate_to_char_boundary limited by byte count — 800 bytes of
multibyte UTF-8 could exceed 500 tokens. New truncate_to_chars
limits by character count (800 chars ≈ 530 tokens)."
```

---

### Task 2: Remove auto-recall and auto-retain from cron

**Files:**
- Modify: `crates/bot/src/cron.rs:180-191` (execute_job signature)
- Modify: `crates/bot/src/cron.rs:301-319` (recall before exec)
- Modify: `crates/bot/src/cron.rs:557-590` (retain + prefetch after exec)
- Modify: `crates/bot/src/cron.rs:691-700` (run_cron_task signature)
- Modify: `crates/bot/src/cron.rs:807-819` (reconcile_jobs signature)
- Modify: `crates/bot/src/cron.rs:956-967` (run_job_loop signature)
- Modify: `crates/bot/src/lib.rs:418-421` (run_cron_task call site)

- [ ] **Step 1: Remove `hindsight` and `prefetch_cache` from `execute_job` signature**

In `crates/bot/src/cron.rs`, remove the last two parameters from `execute_job` (lines 189–190):

```rust
// Old:
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    ssh_config_path: Option<&std::path::Path>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
    hindsight: Option<&Arc<rightclaw::memory::hindsight::HindsightClient>>,
    prefetch_cache: Option<&rightclaw::memory::prefetch::PrefetchCache>,
) {

// New:
async fn execute_job(
    job_name: &str,
    spec: &CronSpec,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: Option<&str>,
    ssh_config_path: Option<&std::path::Path>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
) {
```

- [ ] **Step 2: Replace memory_mode block with `None`**

Replace lines 301–319 (the `memory_mode` block) with:

```rust
    // Cron jobs skip memory injection — cron prompts are static instructions,
    // not user queries. Agents can still call memory_recall/memory_retain MCP
    // tools explicitly from within cron prompts.
    let memory_mode: Option<crate::telegram::prompt::MemoryMode> = None;
```

- [ ] **Step 3: Remove auto-retain and prefetch blocks after cron completion**

Delete lines 557–590 (the entire `if let Some(hs) = hindsight { ... }` block containing auto-retain and prefetch recall). This removes:
- The `tokio::spawn` that calls `hs_retain.retain(...)` 
- The prefetch cache invalidation (`cache_to_clear.clear()`)
- The `tokio::spawn` that calls `hs_recall.recall(...)` for prefetch

- [ ] **Step 4: Remove `hindsight` and `prefetch_cache` from `run_cron_task` signature**

In `crates/bot/src/cron.rs`, update `run_cron_task` (line 691):

```rust
// Old:
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: CancellationToken,
    resolved_sandbox: Option<String>,
    hindsight: Option<Arc<rightclaw::memory::hindsight::HindsightClient>>,
    prefetch_cache: Option<rightclaw::memory::prefetch::PrefetchCache>,
) {

// New:
pub async fn run_cron_task(
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: CancellationToken,
    resolved_sandbox: Option<String>,
) {
```

Remove `&hindsight` and `&prefetch_cache` from both `reconcile_jobs(...)` calls at lines 719 and 724.

- [ ] **Step 5: Remove `hindsight` and `prefetch_cache` from `reconcile_jobs` signature**

Update `reconcile_jobs` (line 807):

```rust
// Old:
fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    triggered_handles: &mut Vec<JoinHandle<()>>,
    conn: &rusqlite::Connection,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Option<String>,
    ssh_config_path: &Option<std::path::PathBuf>,
    internal_client: &Arc<rightclaw::mcp::internal_client::InternalClient>,
    execute_handles: &ExecuteHandles,
    resolved_sandbox: &Option<String>,
    hindsight: &Option<Arc<rightclaw::memory::hindsight::HindsightClient>>,
    prefetch_cache: &Option<rightclaw::memory::prefetch::PrefetchCache>,
) {

// New:
fn reconcile_jobs(
    handles: &mut HashMap<String, (CronSpec, JoinHandle<()>)>,
    triggered_handles: &mut Vec<JoinHandle<()>>,
    conn: &rusqlite::Connection,
    agent_dir: &std::path::Path,
    agent_name: &str,
    model: &Option<String>,
    ssh_config_path: &Option<std::path::PathBuf>,
    internal_client: &Arc<rightclaw::mcp::internal_client::InternalClient>,
    execute_handles: &ExecuteHandles,
    resolved_sandbox: &Option<String>,
) {
```

Inside `reconcile_jobs`, update all three `execute_job(...)` call sites (lines ~858, ~941, ~1014) to remove `hs.as_ref(), pc.as_ref()` arguments. Also remove the clones of `hs` and `pc` that feed into those calls (lines ~855–856, ~900–901, ~936–937, ~1011–1012).

- [ ] **Step 6: Remove `hindsight` and `prefetch_cache` from `run_job_loop` signature**

Update `run_job_loop` (line 956):

```rust
// Old:
async fn run_job_loop(
    job_name: String,
    spec: CronSpec,
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    execute_handles: ExecuteHandles,
    resolved_sandbox: Option<String>,
    hindsight: Option<Arc<rightclaw::memory::hindsight::HindsightClient>>,
    prefetch_cache: Option<rightclaw::memory::prefetch::PrefetchCache>,
) {

// New:
async fn run_job_loop(
    job_name: String,
    spec: CronSpec,
    agent_dir: std::path::PathBuf,
    agent_name: String,
    model: Option<String>,
    ssh_config_path: Option<std::path::PathBuf>,
    internal_client: Arc<rightclaw::mcp::internal_client::InternalClient>,
    execute_handles: ExecuteHandles,
    resolved_sandbox: Option<String>,
) {
```

Update the `execute_job(...)` call inside `run_job_loop` (line ~1014) to remove `hs.as_ref(), pc.as_ref()`. Remove the clones at lines ~1011–1012.

Update the `run_job_loop(...)` spawn in `reconcile_jobs` (line ~904) to remove `job_hindsight, job_prefetch` arguments.

- [ ] **Step 7: Update `lib.rs` call site**

In `crates/bot/src/lib.rs`, remove the hindsight and prefetch clones and arguments from the `run_cron_task` call (lines 418–421):

```rust
// Old:
    let cron_hindsight = hindsight_client.clone();
    let cron_prefetch = prefetch_cache.clone();
    let cron_handle = tokio::spawn(async move {
        cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_model, cron_ssh_config, cron_internal_client, cron_shutdown, cron_sandbox, cron_hindsight, cron_prefetch).await;
    });

// New:
    let cron_handle = tokio::spawn(async move {
        cron::run_cron_task(cron_agent_dir, cron_agent_name, cron_model, cron_ssh_config, cron_internal_client, cron_shutdown, cron_sandbox).await;
    });
```

- [ ] **Step 8: Build to verify compilation**

Run: `devenv shell -- cargo build -p rightclaw-bot 2>&1 | tail -20`
Expected: clean build, no errors. There may be unused import warnings for `Arc` in cron.rs — remove the import if flagged.

- [ ] **Step 9: Run tests**

Run: `devenv shell -- cargo test -p rightclaw-bot 2>&1 | tail -30`
Expected: all tests pass. If any cron tests reference `hindsight` or `prefetch_cache` args, update them to remove those arguments.

- [ ] **Step 10: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/src/lib.rs
git commit -m "fix(cron): remove auto-recall and auto-retain from cron jobs

Cron prompts are static instructions — recall results are irrelevant
and corrupt user memory representations. Auto-retain of cron summaries
is also removed (matching hermes-agent skip_memory=True). Crons can
still call memory_recall/memory_retain MCP tools explicitly.

Removes hindsight and prefetch_cache parameters from the entire cron
call chain: execute_job, run_job_loop, reconcile_jobs, run_cron_task."
```

---

### Task 3: Remove auto-recall from cron delivery

**Files:**
- Modify: `crates/bot/src/cron_delivery.rs:183-194` (run_delivery_loop signature)
- Modify: `crates/bot/src/cron_delivery.rs:342-352` (deliver_through_session signature)
- Modify: `crates/bot/src/cron_delivery.rs:410-427` (recall + memory_mode block)
- Modify: `crates/bot/src/lib.rs:440-452` (run_delivery_loop call site)

- [ ] **Step 1: Remove `hindsight` from `deliver_through_session` signature**

In `crates/bot/src/cron_delivery.rs`, update `deliver_through_session` (line 342):

```rust
// Old:
async fn deliver_through_session(
    yaml_input: &str,
    agent_dir: &Path,
    agent_name: &str,
    bot: &crate::telegram::BotType,
    notify_chat_ids: &[i64],
    ssh_config_path: Option<&Path>,
    session_id: Option<String>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
    hindsight: Option<&std::sync::Arc<rightclaw::memory::hindsight::HindsightClient>>,
) -> Result<(), String> {

// New:
async fn deliver_through_session(
    yaml_input: &str,
    agent_dir: &Path,
    agent_name: &str,
    bot: &crate::telegram::BotType,
    notify_chat_ids: &[i64],
    ssh_config_path: Option<&Path>,
    session_id: Option<String>,
    internal_client: &rightclaw::mcp::internal_client::InternalClient,
    resolved_sandbox: Option<&str>,
) -> Result<(), String> {
```

- [ ] **Step 2: Replace memory_mode block with `None`**

Replace the `memory_mode` block (lines 410–427) with:

```rust
    // Delivery sessions skip memory injection — same rationale as cron jobs.
    let memory_mode: Option<crate::telegram::prompt::MemoryMode> = None;
```

- [ ] **Step 3: Update `deliver_through_session` call site**

In the same file, update the call at line ~283 to remove `hindsight.as_ref()`:

```rust
// Old:
        match deliver_through_session(
            &yaml,
            &agent_dir,
            &agent_name,
            &bot,
            &notify_chat_ids,
            ssh_config_path.as_deref(),
            session_id,
            &internal_client,
            resolved_sandbox.as_deref(),
            hindsight.as_ref(),
        )

// New:
        match deliver_through_session(
            &yaml,
            &agent_dir,
            &agent_name,
            &bot,
            &notify_chat_ids,
            ssh_config_path.as_deref(),
            session_id,
            &internal_client,
            resolved_sandbox.as_deref(),
        )
```

- [ ] **Step 4: Remove `hindsight` from `run_delivery_loop` signature**

Update `run_delivery_loop` (line 183):

```rust
// Old:
pub async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    bot: crate::telegram::BotType,
    notify_chat_ids: Vec<i64>,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: std::sync::Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: tokio_util::sync::CancellationToken,
    resolved_sandbox: Option<String>,
    hindsight: Option<std::sync::Arc<rightclaw::memory::hindsight::HindsightClient>>,
) {

// New:
pub async fn run_delivery_loop(
    agent_dir: PathBuf,
    agent_name: String,
    bot: crate::telegram::BotType,
    notify_chat_ids: Vec<i64>,
    idle_ts: Arc<IdleTimestamp>,
    ssh_config_path: Option<PathBuf>,
    internal_client: std::sync::Arc<rightclaw::mcp::internal_client::InternalClient>,
    shutdown: tokio_util::sync::CancellationToken,
    resolved_sandbox: Option<String>,
) {
```

- [ ] **Step 5: Update `lib.rs` call site**

In `crates/bot/src/lib.rs`, remove the hindsight clone and argument from the `run_delivery_loop` call (lines 440–452):

```rust
// Old:
    let delivery_hindsight = hindsight_client.clone();
    let delivery_handle = tokio::spawn(async move {
        cron_delivery::run_delivery_loop(
            delivery_agent_dir,
            delivery_agent_name,
            delivery_bot,
            delivery_chat_ids,
            delivery_idle_ts,
            delivery_ssh_config,
            delivery_internal_client,
            delivery_shutdown,
            delivery_sandbox,
            delivery_hindsight,

// New:
    let delivery_handle = tokio::spawn(async move {
        cron_delivery::run_delivery_loop(
            delivery_agent_dir,
            delivery_agent_name,
            delivery_bot,
            delivery_chat_ids,
            delivery_idle_ts,
            delivery_ssh_config,
            delivery_internal_client,
            delivery_shutdown,
            delivery_sandbox,
```

- [ ] **Step 6: Build and test**

Run: `devenv shell -- cargo build -p rightclaw-bot 2>&1 | tail -20`
Expected: clean build

Run: `devenv shell -- cargo test -p rightclaw-bot 2>&1 | tail -30`
Expected: all tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cron_delivery.rs crates/bot/src/lib.rs
git commit -m "fix(delivery): remove auto-recall from cron delivery sessions

Delivery is a relay task — recall results are irrelevant. Removes
hindsight parameter from deliver_through_session, run_delivery_loop,
and the lib.rs spawn site."
```

---

### Task 4: Update ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md:387-391`

- [ ] **Step 1: Update the cron memory paragraph**

Replace lines 387–391:

```markdown
**Cron jobs skip memory:** Cron and delivery sessions do not perform recall —
cron prompts are static system instructions, not user queries, so recall results
would be irrelevant and corrupt user memory representations (same approach as
hermes-agent `skip_memory=True`). Auto-retain after cron completion is still
active so cron results can be remembered (plain text summary, no document_id/tags).
```

With:

```markdown
**Cron jobs skip memory:** Cron and delivery sessions perform no auto-recall
or auto-retain. Cron prompts are static instructions — recall results would be
irrelevant and corrupt user memory representations (same approach as hermes-agent
`skip_memory=True`). Crons can call `memory_recall` and `memory_retain` MCP tools
explicitly when needed.
```

Also update line 383 — recall truncation description:

```markdown
Auto-recall before each `claude -p`: query truncated to 800 chars, tags
```

This line already says "800 chars" which is now correct (was bytes before, now actually chars).

- [ ] **Step 2: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: align ARCHITECTURE.md with cron memory changes"
```

---

### Task 5: Full workspace build and test

- [ ] **Step 1: Full build**

Run: `devenv shell -- cargo build --workspace 2>&1 | tail -20`
Expected: clean build, no warnings

- [ ] **Step 2: Full test suite**

Run: `devenv shell -- cargo test --workspace 2>&1 | tail -30`
Expected: all tests pass

- [ ] **Step 3: Check for unused imports**

Run: `devenv shell -- cargo clippy --workspace 2>&1 | grep "unused" | head -20`
Expected: no unused import warnings related to our changes. If any appear (e.g., `Arc` no longer needed in cron.rs, or `rightclaw::memory::hindsight` imports), fix them and amend the relevant commit.
