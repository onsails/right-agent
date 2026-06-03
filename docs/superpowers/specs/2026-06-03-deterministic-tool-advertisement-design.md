# Deterministic tool advertisement — design

- **Date:** 2026-06-03
- **Status:** approved (brainstorm), pending implementation plan
- **Scope:** Spec 2 of 3 from the foreground context-usage audit. Spec 1
  (context placement) is merged. Spec 3 (`get_messages_by_id` + reference
  replies) is separate.

## Problem

The MCP aggregator advertises a per-agent tool list to every `claude -p`
invocation. Two defects make that list **non-deterministic and sometimes
incomplete**, which (1) corrupts the agent's capabilities intermittently and
(2) triggers `cache_miss_reason: tools_changed` — and because the cached
prefix order is `tools → system → messages`, a tools change invalidates the
system prompt and the whole transcript too.

`Aggregator::tools_list` (`crates/right/src/aggregator.rs:483-516`):

```rust
let Some(proxies) = registry.proxies.try_read().ok() else {
    return tools;                       // (b) partial list on lock contention
};
for (proxy_name, handle) in proxies.iter() {   // (a) HashMap iteration order
    if let Some(proxy_tools) = handle.try_tools() {   // (b) silently skips a contended proxy
        ...
    }
}
```

- **(b) Partial list on contention.** `try_read().ok()` returns the list
  *without any proxy tools* when the `proxies` lock is momentarily contended;
  `try_tools()` silently drops an individual proxy whose cache lock is
  contended. This is both a cache-buster (the external MCP tool set flickers
  between turns) and a **correctness bug**: the agent intermittently loses
  access to its external MCP tools mid-session. It is the `if let Err … {}`
  swallow anti-pattern in another shape — a wrong-but-non-erroring result.
- **(a) Non-deterministic order.** Proxy tools are appended in `HashMap`
  iteration order. Stable within one process, but it changes across bot
  restarts and proxy attach/detach, and the upstream server's own tool order
  is not guaranteed. Whether CC re-sorts the API `tools` array is not
  something we should depend on.

### Evidence

The audit found `tools_changed` on the right agent: **35 turns, 3.18M missed
tokens**. The agent runs external (composio) proxy tools, exactly the set
affected by (a)/(b).

### Cross-check: Hermes

Hermes guarantees a stable advertised order with a single `sorted(tool_names)`
in its tool registry (`tools/registry.py:119`), applied uniformly to built-in
and MCP-discovered tools (interleaved alphabetically). Tool definitions are
not cache-marked; their stable order plus position ahead of the system
breakpoint is what keeps the prompt-cache prefix intact. We adopt the same
idea: sort at the source.

## Design

One method changes: `Aggregator::tools_list` becomes `async` and always
returns the complete, name-sorted list. The async accessor already exists —
`ProxyBackend::tools()` (`crates/right-mcp/src/proxy.rs:674`) awaits the
proxy's cached-tools lock — it is simply not used here today.

```rust
pub(crate) async fn tools_list(&self, agent_name: &str) -> Vec<Tool> {
    let Some(registry) = self.agents.get(agent_name) else {
        return Vec::new();
    };

    let mut tools = registry.right.tools_list();
    if registry.hindsight.is_some() {
        tools.extend(HindsightBackend::tools_list());
    }
    tools.push(BackendRegistry::mcp_list_tool_def());

    // (b) Snapshot the proxy handles under the lock, then release it before
    // awaiting each proxy — minimal lock hold, no partial list, no ordering
    // hazard with attach/detach writers.
    let handles: Vec<(String, std::sync::Arc<ProxyBackend>)> = {
        let proxies = registry.proxies.read().await;
        proxies
            .iter()
            .map(|(name, handle)| (name.clone(), handle.clone()))
            .collect()
    };
    for (proxy_name, handle) in &handles {
        for t in handle.tools().await.iter() {
            let mut prefixed = t.clone();
            prefixed.name = Cow::Owned(format!("{proxy_name}__{}", t.name));
            tools.push(prefixed);
        }
    }

    // (a) Canonical order, independent of HashMap iteration, restart order,
    // upstream order, and whether CC re-sorts. Tool names are unique
    // (proxy names are unique map keys; built-ins are unprefixed), so this is
    // a total order.
    tools.sort_by(|a, b| a.name.as_ref().cmp(b.name.as_ref()));
    tools
}
```

**Caller updates** (small blast radius): the `ServerHandler::list_tools` body
(`aggregator.rs:636`, already an `async move` block) gains `.await`; the two
aggregator tests that call `dispatcher.tools_list(...)` (`aggregator.rs:858`,
`:1030`) gain `.await`. `RightBackend::tools_list()` and
`HindsightBackend::tools_list()` are different (sync) methods that build fixed
Vecs — unchanged. `internal_api.rs:936` calls `registry.right.tools_list()`
(the RightBackend method) — unchanged.

### Why async-and-await rather than retry/spin

`tools_list` was sync to keep the handler simple, forcing `try_read`. The
handler is already async, so awaiting the read lock is free of new machinery
and is the only option that *guarantees* a complete list. The lock is held by
writers only during proxy attach/detach (rare and brief); snapshotting the
`Arc` handles and dropping the lock before the per-proxy `tools().await`
keeps the hold minimal. Lock ordering is safe: we take `proxies`(read) then,
after releasing it, each proxy's `cached_tools`(read); no path takes them in
the inverse order.

## What this does NOT change (and why)

- **Reflection / async-delivery tool sets stay stripped.** These resume the
  foreground session without forking (`reflection.rs` `fork_session:false`;
  `async_delivery.rs` `fork_session:false`) and deliberately strip
  foreground-only tools (and reflection strips `Agent`). Aligning them to the
  foreground tool set to win cache would mean offering tools they are denied
  for safety/cost — `Agent` to a budget-capped reflection turn
  (runaway-subagent risk), and progress/search/forum/learning tools to a
  relay where they error or cause side effects. That trades a safety boundary
  for a cache optimization, and `Agent` is a CC built-in that cannot be
  "advertised but server-rejected", so reflection cannot be both safe and
  coherent anyway. Since reflection (on failure) and delivery (on background
  completion) are infrequent, the occasional miss is acceptable. Out of scope.
- **Background continuations** fork to a new `run_id` session
  (`background.rs` `fork_session:true`), so they do not poison the foreground
  session's cache. Not an offender.

## Testing

- New unit test: register a `ProxyBackend` whose cached tools are in
  non-alphabetical order; assert `tools_list(agent).await` returns all of
  them, prefixed, and the full result is sorted by name (built-ins + meta +
  proxy interleaved alphabetically).
- New unit test: two proxies with overlapping bare tool names; assert both
  appear with distinct `{server}__name` prefixes and the list is sorted.
- The "never partial on contention" property is correctness-by-construction
  (`read().await` is unconditional); no timing-based contention test (flaky).
- Update existing aggregator tests that assert tool *membership*
  (`tools_list_includes_right_and_meta`) for the `.await`; if any asserts a
  specific index/order, update it to the sorted order.

## Upgrade & compatibility

Aggregator-internal; no on-disk format, no migration, no sandbox change.
Takes effect when the aggregator process restarts on `right up` / restart.
Fully backward-compatible.

## Verification cadence

- TDD: write the sort/membership tests first; verify they fail (sync arity /
  unsorted), then implement.
- Targeted: `devenv shell -- cargo test -p right aggregator` (and
  `-p right-mcp` if the proxy accessor is touched — it is not).
- Final, mandatory: `devenv shell -- cargo test --workspace` from the
  worktree before declaring complete.

## Open questions

None — scope resolved in brainstorming (option A: aggregator determinism
only; reflection/delivery coherence and the advertise-but-server-reject
mechanism are explicitly deferred).
