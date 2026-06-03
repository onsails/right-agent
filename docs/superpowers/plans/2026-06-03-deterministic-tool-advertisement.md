# Deterministic Tool Advertisement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the MCP aggregator advertise a complete, name-sorted tool list to every `claude -p` invocation, eliminating the partial-list-on-contention correctness bug and the resulting spurious `tools_changed` cache misses.

**Architecture:** One method changes — `ToolDispatcher::tools_list` (`crates/right/src/aggregator.rs`) becomes `async`, snapshots proxy handles under the `proxies` read lock and then awaits each proxy's already-async `tools()` (so a contended lock never yields a partial list), and sorts the assembled list by tool name before returning. Three call sites add `.await`.

**Tech Stack:** Rust (edition 2024), tokio (`RwLock`), rmcp `ServerHandler`. Crate: `right`. Targeted tests: `cargo test -p right aggregator`. Spec: `docs/superpowers/specs/2026-06-03-deterministic-tool-advertisement-design.md`.

**Conventions:** FAIL FAST — the change replaces a silent partial-result (`try_read().ok() → return`) with an unconditional `read().await`. No `std::env::set_var` in tests. Final `devenv shell -- cargo test --workspace` is mandatory.

---

## File Structure

- `crates/right/src/aggregator.rs` — `ToolDispatcher::tools_list` (lines 483-516): async + await + sort. `ServerHandler::list_tools` body (line 636): add `.await`. Test module: convert the two `tools_list`-calling tests to `#[tokio::test]` + `.await`, add a sort test.

No new files. `ProxyBackend`, `Arc`, and `Cow` are already imported in `aggregator.rs` (lines 15, ~882, 509).

---

## Task 1: Make `tools_list` async, await locks, sort by name

**Files:**
- Modify: `crates/right/src/aggregator.rs:483-516` (`ToolDispatcher::tools_list`)
- Modify: `crates/right/src/aggregator.rs:636` (`list_tools` call site)
- Test: `crates/right/src/aggregator.rs` (test module, ~line 851)

- [ ] **Step 1: Write the failing/updated tests**

In the `aggregator.rs` test module, convert the existing membership test to async and add a sort test. Replace `tools_list_includes_right_and_meta` (lines 853-876) with:

```rust
    #[tokio::test]
    async fn tools_list_includes_right_and_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());

        let tools = dispatcher.tools_list("test-agent").await;
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();

        assert!(names.contains(&"cron_create"), "missing cron_create");
        assert!(
            names.contains(&crate::progress::SEND_PROGRESS_TOOL),
            "missing send_progress"
        );
        assert!(names.contains(&"thread_search"), "missing thread_search");
        assert!(names.contains(&"chat_search"), "missing chat_search");
        assert!(names.contains(&"bootstrap_done"), "missing bootstrap_done");
        assert!(
            names.contains(&"rightmeta__mcp_list"),
            "missing rightmeta__mcp_list"
        );
    }

    #[tokio::test]
    async fn tools_list_is_sorted_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());

        let tools = dispatcher.tools_list("test-agent").await;
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "advertised tools must be in canonical (sorted) order"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `devenv shell -- cargo test -p right tools_list_is_sorted_by_name tools_list_includes_right_and_meta`
Expected: FAIL to compile — `.await` applied to the still-sync `tools_list` (`tools_list` is not a future). This confirms the tests bind to the new async contract.

- [ ] **Step 3: Make `tools_list` async + await + sort**

Replace `ToolDispatcher::tools_list` (lines 483-516) with:

```rust
    pub(crate) async fn tools_list(&self, agent_name: &str) -> Vec<Tool> {
        let Some(registry) = self.agents.get(agent_name) else {
            return Vec::new();
        };

        let mut tools = registry.right.tools_list();

        if registry.hindsight.is_some() {
            tools.extend(HindsightBackend::tools_list());
        }

        // rightmeta__mcp_list
        tools.push(BackendRegistry::mcp_list_tool_def());

        // Snapshot proxy handles under the read lock, then release it before
        // awaiting each proxy. Awaiting (not try_read) guarantees a COMPLETE
        // list — a contended lock must never silently drop proxy tools (that
        // is both a cache-buster and a correctness bug: the agent would lose
        // its external MCP tools mid-session).
        let handles: Vec<(String, Arc<ProxyBackend>)> = {
            let proxies = registry.proxies.read().await;
            proxies
                .iter()
                .map(|(name, handle)| (name.clone(), handle.clone()))
                .collect()
        };
        for (proxy_name, handle) in &handles {
            for t in handle.tools().await.iter() {
                let prefixed_name = format!("{proxy_name}__{}", t.name);
                let mut prefixed = t.clone();
                prefixed.name = Cow::Owned(prefixed_name);
                tools.push(prefixed);
            }
        }

        // Canonical order, independent of HashMap iteration, restart order,
        // upstream order, and whether CC re-sorts. Tool names are unique
        // (proxy names are unique map keys; built-ins are unprefixed).
        tools.sort_by(|a, b| a.name.as_ref().cmp(b.name.as_ref()));
        tools
    }
```

- [ ] **Step 4: Update the production call site**

In `ServerHandler::list_tools` (`aggregator.rs:636`), inside the existing `async move` block, add `.await`:

```rust
            let tools = self.dispatcher.tools_list(&agent.name).await;
```

- [ ] **Step 5: Run the two tests to verify they pass**

Run: `devenv shell -- cargo test -p right tools_list_is_sorted_by_name tools_list_includes_right_and_meta`
Expected: PASS. (If other tests in the file still fail to compile because they call the now-async `tools_list`, that is fixed in Task 2 — those won't be selected by this filter, but a package compile is required, so proceed to Task 2 before a package-wide run.)

- [ ] **Step 6: Commit**

```bash
git add crates/right/src/aggregator.rs
git commit -m "fix(aggregator): complete, name-sorted tool list (no partial list on contention)"
```

---

## Task 2: Update remaining async call site(s) and verify the package

**Files:**
- Modify: `crates/right/src/aggregator.rs` (any other test calling `dispatcher.tools_list(...)`, e.g. ~line 1030)

- [ ] **Step 1: Find all remaining `tools_list` dispatcher call sites**

Run: `rg -n 'dispatcher\.tools_list\(|\.tools_list\("test-agent"\)' crates/right/src/aggregator.rs`
Expected: the prod site (line 636, already `.await` from Task 1), the two tests updated in Task 1, and one more test around line 1030 (e.g. `all_tools_have_valid_input_schema`).

- [ ] **Step 2: Convert that test to async + await**

For the remaining test (around line 1027-1030), change its attribute from `#[test]` to `#[tokio::test]`, make the fn `async`, and add `.await` to the `tools_list` call:

```rust
    #[tokio::test]
    async fn all_tools_have_valid_input_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let dispatcher = make_dispatcher(tmp.path());
        let tools = dispatcher.tools_list("test-agent").await;
        // ...rest of the existing assertions unchanged...
    }
```

(If the test name differs, apply the same three edits — `#[tokio::test]`, `async fn`, `.await` — to whichever test calls `dispatcher.tools_list`.)

- [ ] **Step 3: Build + test the package**

Run: `devenv shell -- cargo test -p right aggregator`
Expected: PASS — all aggregator tests compile and pass. Fix any straggler call site the build flags (`tools_list` used without `.await`).

- [ ] **Step 4: Clippy**

Run: `devenv shell -- cargo clippy -p right -- -D warnings`
Expected: clean. (No `manual_async_fn` issue: `tools_list` is a plain `async fn`, not a trait-signature method.)

- [ ] **Step 5: Commit**

```bash
git add crates/right/src/aggregator.rs
git commit -m "test(aggregator): await async tools_list in remaining tests"
```

---

## Task 3: Final workspace verification

- [ ] **Step 1: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. (Per the flaky-tests note, re-run any cc/invocation pid-race or dashboard warn-count failure in isolation before attributing it to this change.)

- [ ] **Step 2: Workspace build**

Run: `devenv shell -- cargo build --workspace`
Expected: success.

- [ ] **Step 3: Optional manual cache check**

For an agent with at least one external (proxy) MCP server, run two consecutive foreground turns and confirm in the stream NDJSON that the second turn shows no `cache_miss_reason: tools_changed` and that the proxy tools are consistently present in the `system` init `tools` array across turns.

---

## Self-Review

**Spec coverage:**
- (b) no partial list on contention → Task 1 Step 3 (`read().await` + `handle.tools().await`, replacing `try_read().ok()`/`try_tools()`).
- (a) deterministic order → Task 1 Step 3 (`sort_by` on `name`), tested by `tools_list_is_sorted_by_name`.
- async conversion + call sites → Task 1 Step 4 (prod), Task 2 (tests).
- "does NOT change reflection/delivery/background" → no task touches them (correct; out of scope).
- upgrade/compat (aggregator-internal) → no migration task needed (correct).
- testing (sort + membership; no flaky contention test) → Task 1 Steps 1-5; proxy-construction test intentionally omitted (impractical in the `right` crate — `ProxyBackend::new` needs a client and `cached_tools` is private cross-crate; the sort applies to the whole assembled vec by construction, and the built-in Vec is non-alphabetical so the sort test is meaningful).
- final workspace test → Task 3.

**Placeholder scan:** none. The only conditional ("if the test name differs", "if other tests fail to compile") gives the exact `rg` command and the exact three edits to apply.

**Type consistency:** `tools_list(&self, agent_name: &str) -> Vec<Tool>` becomes `async`; callers use `.await` uniformly (Task 1 Step 4, Task 2 Step 2). `handle.tools().await -> Vec<Tool>` (`proxy.rs:674`) and `Arc<ProxyBackend>` (`proxies` value type) match. `sort_by` uses `a.name.as_ref().cmp(b.name.as_ref())` consistently.
