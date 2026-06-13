# Inline Skill Authoring + Cron "What, Not How" — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the agent author/patch `rightx-*` skills mid-conversation on its own judgment (foreground + cron), skip the async probe when it does, and teach crons to carry the "what" while skills carry the "how".

**Architecture:** Mostly reuses existing learning seams. Foreground inline authoring already works at runtime (per-invocation MCP config + learning-capable `Foreground` kind) — it's gated only by skill text. Cron gains a learning-capable `Cron` invocation kind so its inline writes pass `skill_learning_start`. A new `ProbeAnchor.learning_invocation_id` lets `run_post_turn` skip the async probe whenever a skill was authored/patched that turn (detected via the existing `successful_finish_exists`). The curator's auto-lifecycle widens to cover `foreground`+`cron` so liberal authoring can't bloat the skill index.

**Tech Stack:** Rust (edition 2024), `turso`/`right-db` SQLite, tokio, Claude Code `claude -p`, OpenShell sandbox. Tests via `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-06-13-inline-skill-authoring-design.md`

**Verification cadence (per AGENTS.md):** targeted `cargo nextest run -p <crate> <filter>` per task; one full `cargo nextest run --workspace` + `cargo test --doc --workspace` at the end (Task 12). All commands run under `devenv shell --`.

---

## File Structure

Touched files, by responsibility:

- `crates/right-lifecycle/src/lib.rs` — `CreatedBy::Cron`; widen curator lifecycle queries.
- `crates/right-dashboard/src/api_types.rs` — `SkillCreatedBy::Cron` + `From`.
- `crates/right-mcp/src/internal_client.rs` — `ProgressInvocationKindDto::Cron`.
- `crates/right/src/progress.rs` — `ProgressInvocationKind::Cron` + learning-capability.
- `crates/right/src/internal_api.rs` — DTO→kind conversion.
- `crates/right/src/right_backend.rs` — `invocation_kind_to_created_by` Cron arm.
- `crates/bot/src/telegram/worker.rs` — `ProbeAnchor.learning_invocation_id`; `CcReply` threading; foreground anchor.
- `crates/bot/src/cron.rs` — register `Cron` invocation; thread invocation_id; cleanup.
- `crates/bot/src/cc/invocation.rs` — `disallow_foreground_only_tools_keep_learning` helper.
- `crates/bot/src/learning_pipeline.rs` — `authored_skill_this_turn` skip in `run_post_turn`.
- `crates/right-codegen/skills/right-learn-skill/SKILL.md` — relax gate (Component 2).
- `crates/right-codegen/skills/right-cron/SKILL.md` — what-not-how (Component 1).
- `crates/right-codegen/src/agent_def_tests.rs` — update the description-framing test.
- `docs/architecture/learning.md`, `PROMPT_SYSTEM.md`, `ARCHITECTURE.md` — docs.

---

## Task 1: `CreatedBy::Cron` provenance

**Files:**
- Modify: `crates/right-lifecycle/src/lib.rs:37-63` (enum + `as_db_str` + `from_db_str`)
- Modify: `crates/right-dashboard/src/api_types.rs` (`SkillCreatedBy` enum + `From<CreatedBy>`)
- Test: `crates/right-lifecycle/src/lib.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/right-lifecycle/src/lib.rs`:

```rust
#[test]
fn created_by_cron_round_trips_db_str() {
    assert_eq!(CreatedBy::Cron.as_db_str(), "cron");
    assert_eq!(CreatedBy::from_db_str("cron").unwrap(), CreatedBy::Cron);
}
```

- [ ] **Step 2: Run it — expect FAIL (no `Cron` variant)**

Run: `devenv shell -- cargo nextest run -p right-lifecycle created_by_cron_round_trips_db_str`
Expected: compile error `no variant named Cron`.

- [ ] **Step 3: Add the variant and string mapping**

In `crates/right-lifecycle/src/lib.rs`, extend the enum (after `Bundled,`) and both match arms:

```rust
pub enum CreatedBy {
    Foreground,
    ProbeWriter,
    Curator,
    Bundled,
    Cron,
}
```

In `as_db_str`, add arm: `Self::Cron => "cron",`
In `from_db_str`, add arm before `other =>`: `"cron" => Ok(Self::Cron),`

- [ ] **Step 4: Update the dashboard mirror so the workspace compiles**

In `crates/right-dashboard/src/api_types.rs`, add `Cron,` to the `SkillCreatedBy` enum and add to the `From<right_lifecycle::CreatedBy>` match:

```rust
right_lifecycle::CreatedBy::Cron => Self::Cron,
```

- [ ] **Step 5: Run the test — expect PASS**

Run: `devenv shell -- cargo nextest run -p right-lifecycle created_by_cron_round_trips_db_str`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/right-lifecycle/src/lib.rs crates/right-dashboard/src/api_types.rs
git commit -m "feat(lifecycle): add cron skill provenance"
```

---

## Task 2: Widen curator auto-lifecycle to `foreground` + `cron`

**Files:**
- Modify: `crates/right-lifecycle/src/lib.rs:285-298` (`list_curator_candidates`)
- Modify: `crates/right-lifecycle/src/lib.rs:300-359` (`apply_automatic_transitions`)
- Test: `crates/right-lifecycle/src/lib.rs` (inline)

This implements spec decision **A**: agent-self-authored skills age out like probe skills; pin is the durability escape.

- [ ] **Step 1: Write the failing test**

Add to the test module. (`LifecycleRowFixture` and `mark_created` already exist in this module — see existing tests around line 480-612.)

```rust
#[tokio::test]
async fn foreground_and_cron_rows_become_curator_candidates() {
    let conn = test_conn().await; // existing helper used by sibling tests
    let now = Utc::now();
    // unused, unpinned, stale rows of each provenance
    for (name, by) in [
        ("rightx-fg", CreatedBy::Foreground),
        ("rightx-cr", CreatedBy::Cron),
        ("rightx-pw", CreatedBy::ProbeWriter),
    ] {
        LifecycleRowFixture::new(name)
            .state(LifecycleState::Stale)
            .created_by(by)
            .insert(&conn)
            .await;
    }
    let candidates = list_curator_candidates(&conn).await.unwrap();
    let names: Vec<_> = candidates.iter().map(|r| r.skill_name.as_str()).collect();
    assert!(names.contains(&"rightx-fg"));
    assert!(names.contains(&"rightx-cr"));
    assert!(names.contains(&"rightx-pw"));
}
```

If the sibling tests use a different connection/fixture constructor than `test_conn()` / `LifecycleRowFixture::insert`, mirror exactly what the nearest existing `#[tokio::test]` in this module uses (read lines 480-560 first and copy the established setup).

- [ ] **Step 2: Run it — expect FAIL**

Run: `devenv shell -- cargo nextest run -p right-lifecycle foreground_and_cron_rows_become_curator_candidates`
Expected: FAIL — `rightx-fg` and `rightx-cr` absent (today's filter is `IN ('probe_writer','curator')`).

- [ ] **Step 3: Widen `list_curator_candidates`**

Change the `WHERE` clause:

```rust
         WHERE state = 'stale'
           AND pinned = 0
           AND created_by IN ('probe_writer', 'curator', 'foreground', 'cron')
         ORDER BY skill_name",
```

- [ ] **Step 4: Widen `apply_automatic_transitions`**

Both `UPDATE`s use positional params `?5/?6` (archive) and `?3/?4` (stale) for the two `created_by` values. Switch each `created_by IN (?, ?)` to a literal 4-value list and drop those positional params, renumbering the cutoff placeholder. Archive `UPDATE`:

```rust
    let archived = tx
        .execute(
            &format!(
                "UPDATE skill_lifecycle
             SET state = ?1, archived_at = ?2
             WHERE state IN (?3, ?4)
               AND pinned = 0
               AND created_by IN ('probe_writer', 'curator', 'foreground', 'cron')
               AND {}",
                activity_before_cutoff("?5"),
            ),
            (
                LifecycleState::Archived.as_db_str(),
                now.as_str(),
                LifecycleState::Active.as_db_str(),
                LifecycleState::Stale.as_db_str(),
                archive_cutoff.as_str(),
            ),
        )
        .await?;
```

Stale `UPDATE`:

```rust
    let staled = tx
        .execute(
            &format!(
                "UPDATE skill_lifecycle
             SET state = ?1
             WHERE state = ?2
               AND pinned = 0
               AND created_by IN ('probe_writer', 'curator', 'foreground', 'cron')
               AND {}",
                activity_before_cutoff("?3"),
            ),
            (
                LifecycleState::Stale.as_db_str(),
                LifecycleState::Active.as_db_str(),
                stale_cutoff.as_str(),
            ),
        )
        .await?;
```

Update the doc comment above the archive block: replace "curator/probe-writer row" with "learned (probe-writer/curator/foreground/cron) row".

- [ ] **Step 5: Add a stale→archive transition test for the new provenance**

```rust
#[tokio::test]
async fn unused_foreground_row_archives_but_pinned_does_not() {
    let conn = test_conn().await;
    let now = Utc::now();
    let old = now - chrono::Duration::days(400);
    LifecycleRowFixture::new("rightx-fg-old")
        .state(LifecycleState::Active)
        .created_by(CreatedBy::Foreground)
        .last_used_at(old)
        .insert(&conn)
        .await;
    LifecycleRowFixture::new("rightx-fg-pinned")
        .state(LifecycleState::Active)
        .created_by(CreatedBy::Foreground)
        .pinned(true)
        .last_used_at(old)
        .insert(&conn)
        .await;
    apply_automatic_transitions(&conn, now, TransitionConfig::default()).await.unwrap();
    let rows = list_all(&conn).await.unwrap(); // or the existing list helper
    let fg = rows.iter().find(|r| r.skill_name == "rightx-fg-old").unwrap();
    let pinned = rows.iter().find(|r| r.skill_name == "rightx-fg-pinned").unwrap();
    assert_eq!(fg.state, LifecycleState::Archived);
    assert_eq!(pinned.state, LifecycleState::Active);
}
```

Use the same fixture builder methods the sibling tests use (`.pinned`, `.last_used_at`, `.state` — confirm exact names against lines 480-560; `TransitionConfig::default()` may need explicit `stale_after`/`archive_after` — copy from an existing transition test).

- [ ] **Step 6: Run both tests — expect PASS**

Run: `devenv shell -- cargo nextest run -p right-lifecycle curator`
Expected: PASS.

- [ ] **Step 7: Check no other test pinned the old behavior**

Run: `devenv shell -- cargo nextest run -p right-lifecycle -p bot apply_automatic_transitions`
Expected: PASS (fix any sibling test that asserted foreground rows are NOT archived — update it to the new behavior, since this is the intended change).

- [ ] **Step 8: Commit**

```bash
git add crates/right-lifecycle/src/lib.rs
git commit -m "feat(lifecycle): auto-manage foreground and cron learned skills"
```

---

## Task 3: `Cron` learning-capable invocation kind

**Files:**
- Modify: `crates/right-mcp/src/internal_client.rs:544-551` (`ProgressInvocationKindDto`)
- Modify: `crates/right/src/progress.rs:32-49` (`ProgressInvocationKind` + impls)
- Modify: `crates/right/src/internal_api.rs` (DTO→kind match)
- Modify: `crates/right/src/right_backend.rs:367-392` (`invocation_kind_to_created_by`)
- Test: `crates/right/src/progress.rs` (inline)

- [ ] **Step 1: Write the failing test**

In the test module of `crates/right/src/progress.rs`:

```rust
#[test]
fn cron_kind_is_learning_capable_but_does_not_send_messages() {
    assert!(ProgressInvocationKind::Cron.is_learning_capable());
    assert!(!ProgressInvocationKind::Cron.sends_learning_messages());
}
```

- [ ] **Step 2: Run it — expect FAIL (no `Cron`)**

Run: `devenv shell -- cargo nextest run -p right cron_kind_is_learning_capable`
Expected: compile error.

- [ ] **Step 3: Add `Cron` to the DTO**

In `crates/right-mcp/src/internal_client.rs`:

```rust
pub enum ProgressInvocationKindDto {
    Foreground,
    BackgroundReview,
    ProbeWriter,
    Curator,
    Cron,
}
```

(`#[serde(rename_all = "snake_case")]` serializes it as `"cron"`.)

- [ ] **Step 4: Add `Cron` to the server kind + capability**

In `crates/right/src/progress.rs`:

```rust
pub(crate) enum ProgressInvocationKind {
    Foreground,
    BackgroundReview,
    ProbeWriter,
    Curator,
    Cron,
    #[cfg(test)]
    NonForeground,
}
```

```rust
    pub(crate) fn is_learning_capable(self) -> bool {
        matches!(
            self,
            Self::Foreground | Self::ProbeWriter | Self::Curator | Self::Cron
        )
    }
```

`sends_learning_messages` stays `matches!(self, Self::Foreground)` — Cron must NOT send (no live user).

- [ ] **Step 5: Map the DTO→kind conversion**

In `crates/right/src/internal_api.rs`, add to the `match req.kind` block:

```rust
    ProgressInvocationKindDto::Cron => crate::progress::ProgressInvocationKind::Cron,
```

- [ ] **Step 6: Map kind→provenance**

In `crates/right/src/right_backend.rs` `invocation_kind_to_created_by`, add before the `BackgroundReview` arm:

```rust
        crate::progress::ProgressInvocationKind::Cron => Ok(right_lifecycle::CreatedBy::Cron),
```

- [ ] **Step 7: Add a DTO-serialization test**

In `crates/right-mcp/src/internal_client.rs` test module:

```rust
#[test]
fn cron_kind_dto_serializes_snake_case() {
    let json = serde_json::to_value(ProgressInvocationKindDto::Cron).unwrap();
    assert_eq!(json, serde_json::json!("cron"));
}
```

- [ ] **Step 8: Run the tests — expect PASS**

Run: `devenv shell -- cargo nextest run -p right -p right-mcp cron_kind`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/right-mcp/src/internal_client.rs crates/right/src/progress.rs crates/right/src/internal_api.rs crates/right/src/right_backend.rs
git commit -m "feat(learning): add learning-capable Cron invocation kind"
```

---

## Task 4: Probe-skip on same-turn authoring

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:208-225` (`ProbeAnchor` field)
- Modify: `crates/bot/src/telegram/worker.rs:2658-2674` (`CcReply` field)
- Modify: `crates/bot/src/telegram/worker.rs:3063` (capture id in `invoke_cc`)
- Modify: all `CcReply { … }` constructions: lines 3487, 4197, 4286, 4297, 4329, 4339, 4471
- Modify: `crates/bot/src/telegram/worker.rs:1694` (destructure) and `:1951` (anchor)
- Modify: `crates/bot/src/cron.rs:977-999` (anchor sets `None` for now)
- Modify: `crates/bot/src/learning_pipeline.rs:42` (`run_post_turn` skip)
- Test: `crates/bot/src/learning_pipeline.rs` (inline)

Detection uses the existing `right_agent::learned_skills::successful_finish_exists(conn, invocation_id)`.

- [ ] **Step 1: Write the failing test (the skip predicate)**

Add to the test module of `crates/bot/src/learning_pipeline.rs`:

```rust
#[tokio::test]
async fn authored_skill_this_turn_true_after_successful_finish() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut c = right_db::open_connection(dir.path(), true).await.unwrap();
        right_db::migrations::MIGRATIONS.to_latest(&mut c).await.unwrap();
    }
    let conn = right_db::open_connection(dir.path(), false).await.unwrap();
    right_agent::learned_skills::insert_learning_event(
        &conn,
        &right_agent::learned_skills::LearningEvent {
            invocation_id: "inv-1".into(),
            agent_name: "a".into(),
            action: right_agent::learned_skills::LearningAction::Create,
            skill_name: "rightx-x".into(),
            phase: right_agent::learned_skills::LearningPhase::Finish,
            status: Some(right_agent::learned_skills::LearningStatus::Created),
            hint_outcome: None, reason: None, message: None, summary: None,
            event_refs: vec![],
        },
    ).await.unwrap();

    assert!(authored_skill_this_turn(&conn, Some("inv-1")).await);
    assert!(!authored_skill_this_turn(&conn, Some("inv-2")).await);
    assert!(!authored_skill_this_turn(&conn, None).await);
}
```

- [ ] **Step 2: Run it — expect FAIL (`authored_skill_this_turn` undefined)**

Run: `devenv shell -- cargo nextest run -p bot authored_skill_this_turn`
Expected: compile error.

- [ ] **Step 3: Add the helper + the skip in `run_post_turn`**

In `crates/bot/src/learning_pipeline.rs`, add the helper:

```rust
/// True when a `rightx-*` skill was successfully created/updated during this
/// turn (so the async probe must not run — the agent already captured the how).
/// `None` invocation (progress/learning disabled) → false.
pub(crate) async fn authored_skill_this_turn(
    conn: &right_db::Connection,
    learning_invocation_id: Option<&str>,
) -> bool {
    let Some(inv) = learning_invocation_id else {
        return false;
    };
    match right_agent::learned_skills::successful_finish_exists(conn, inv).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("learning pipeline: successful_finish_exists failed: {e:#}");
            false
        }
    }
}
```

Then, in `run_post_turn`, immediately after the `conn` is opened (after the `let conn = match … return; };` block, before `today_spend`):

```rust
    if authored_skill_this_turn(&conn, anchor.learning_invocation_id.as_deref()).await {
        tracing::debug!(
            agent = %ctx.agent_name,
            "learning pipeline skipped: skill authored/patched this turn"
        );
        return;
    }
```

- [ ] **Step 4: Add the `learning_invocation_id` field to `ProbeAnchor`**

In `crates/bot/src/telegram/worker.rs`, add to the struct (after `used_skill_receipts`):

```rust
    /// The learning invocation id (the per-invocation MCP config's
    /// `X-Right-Invocation`) for this turn, when one was registered. Used by
    /// the post-turn pipeline to skip the probe if the agent authored/patched a
    /// skill this turn. `None` when no learning invocation was registered.
    pub learning_invocation_id: Option<String>,
```

- [ ] **Step 5: Thread the id through `CcReply`**

Add to the `CcReply` struct (after `wall_elapsed_ms`):

```rust
    /// Learning invocation id used for this turn (for probe-skip), if any.
    pub(crate) learning_invocation_id: Option<String>,
```

In `invoke_cc`, immediately after line 3063 (`let mut active_progress = …;`) add:

```rust
    let learning_invocation_id = active_progress
        .as_ref()
        .map(|active| active.invocation_id.clone());
```

In EACH `CcReply { … }` construction (lines 3487, 4197, 4286, 4297, 4329, 4339, 4471), add the field:

```rust
        learning_invocation_id: learning_invocation_id.clone(),
```

- [ ] **Step 6: Read the field in `spawn_worker` and set the anchor**

At the `Ok(CcReply { … })` destructure (line 1694), add `learning_invocation_id,` to the bound fields. At the anchor construction (line 1951), add:

```rust
                            learning_invocation_id,
```

(Use the bound variable directly; it is consumed once here.)

- [ ] **Step 7: Set `None` on the cron anchor (for now)**

In `crates/bot/src/cron.rs` anchor construction (line 977-999), add the field:

```rust
                                        learning_invocation_id: None,
```

(Task 6 replaces this with the registered cron invocation id.)

- [ ] **Step 8: Build + run the helper test — expect PASS**

Run: `devenv shell -- cargo nextest run -p bot authored_skill_this_turn`
Expected: PASS. Also `devenv shell -- cargo build -p bot` to confirm all `CcReply` sites compile.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/telegram/worker.rs crates/bot/src/cron.rs crates/bot/src/learning_pipeline.rs
git commit -m "feat(learning): skip async probe when a skill was authored this turn"
```

---

## Task 5: Learning-preserving disallowed-tools helper

**Files:**
- Modify: `crates/bot/src/cc/invocation.rs:108-112` (`disallow_foreground_only_tools`)
- Test: `crates/bot/src/cc/invocation.rs` (inline)

`disallow_foreground_only_tools` currently bundles `disallow_learning_tools`, so cron blocks `skill_learning_*`. Split out a learning-preserving variant; keep the original behavior identical (DRY).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn keep_learning_variant_allows_learning_tools_but_disallows_others() {
    let kept = disallow_foreground_only_tools_keep_learning(baseline_disallowed_tools());
    assert!(!kept.iter().any(|t| t
        == right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL));
    assert!(!kept.iter().any(|t| t
        == right_mcp::internal_client::SKILL_LEARNING_FINISH_MCP_TOOL));
    // still disallows send_progress (a foreground-only tool)
    assert!(kept.iter().any(|t| t == SEND_PROGRESS_MCP_TOOL));
    // the full variant still disallows learning tools (unchanged behavior)
    let full = disallow_foreground_only_tools(baseline_disallowed_tools());
    assert!(full.iter().any(|t| t
        == right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL));
}
```

- [ ] **Step 2: Run it — expect FAIL (helper undefined)**

Run: `devenv shell -- cargo nextest run -p bot keep_learning_variant`
Expected: compile error.

- [ ] **Step 3: Refactor the helper (DRY)**

Replace `disallow_foreground_only_tools` (lines 108-112) with:

```rust
/// Foreground-only tool restrictions EXCEPT learning tools. Used by cron turns
/// that may author skills inline.
pub(crate) fn disallow_foreground_only_tools_keep_learning(tools: Vec<String>) -> Vec<String> {
    disallow_thread_focus_set(disallow_forum_topic_tools(disallow_conversation_search(
        disallow_send_progress(tools),
    )))
}

pub(crate) fn disallow_foreground_only_tools(tools: Vec<String>) -> Vec<String> {
    disallow_learning_tools(disallow_foreground_only_tools_keep_learning(tools))
}
```

(Set membership is order-independent, so the full variant's output is unchanged.)

- [ ] **Step 4: Run the test — expect PASS**

Run: `devenv shell -- cargo nextest run -p bot keep_learning_variant`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/bot/src/cc/invocation.rs
git commit -m "refactor(cc): learning-preserving disallowed-tools helper for cron"
```

---

## Task 6: Cron inline authoring wiring

**Files:**
- Modify: `crates/bot/src/cron.rs:472-485` (`execute_job` — register + cleanup)
- Modify: `crates/bot/src/cron.rs:548-580` (disallowed set + mcp path + invocation)
- Modify: `crates/bot/src/cron.rs:977-999` (anchor id)
- Test: live `#[ignore = "ci-claude: …"]` test + targeted unit on the gating decision

`execute_job` has `agent_name`, `agent_dir`, `ssh_config_path`, `internal_client`, `resolved_sandbox`, `learning`, `model` in scope. Register a `Cron` learning invocation when learning is enabled, point the cron CC at its per-invocation MCP config, preserve learning tools, clean up after the run, and stamp the anchor.

- [ ] **Step 1: Register the invocation and choose the disallowed set**

Replace the `disallowed_tools` + `mcp_path` block (lines 548-563) with:

```rust
    let learning_inline_enabled = learning.prefilter_enabled;

    let registered_learning = if learning_inline_enabled {
        match crate::cc::invocation::register_non_foreground_invocation(
            crate::cc::invocation::NonForegroundInvocationRegistration {
                agent_name: agent_name.to_owned(),
                agent_dir: agent_dir.to_path_buf(),
                ssh_config_path: ssh_config_path.map(|p| p.to_path_buf()),
                resolved_sandbox: resolved_sandbox.map(|s| s.to_owned()),
                internal_client: Arc::clone(internal_client),
                kind: right_mcp::internal_client::ProgressInvocationKindDto::Cron,
                chat_id: spec.target_chat_id,
                thread_id: spec.target_thread_id,
            },
        )
        .await
        {
            Ok(active) => Some(active),
            Err(e) => {
                tracing::warn!(job = %job_name, "cron learning invocation register failed: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let disallowed_tools = if registered_learning.is_some() {
        crate::cc::invocation::disallow_foreground_only_tools_keep_learning(
            crate::cc::invocation::baseline_disallowed_tools(),
        )
    } else {
        crate::cc::invocation::disallow_foreground_only_tools(
            crate::cc::invocation::baseline_disallowed_tools(),
        )
    };

    let mcp_path = match &registered_learning {
        Some(active) => active.mcp_config_path().to_owned(),
        None => crate::cc::invocation::mcp_config_path(ssh_config_path, agent_dir),
    };
```

(The `prompt_for_cc` block between the old `disallowed_tools` and `mcp_path` stays where it is — keep it; only the `disallowed_tools` and `mcp_path` bindings move/changed.)

- [ ] **Step 2: Clean up the invocation after the run**

`RegisteredNonForegroundInvocation` is `#[must_use]` and only `cleanup().await` unregisters (Drop just removes files). After the cron CC run completes and the result/outcome has been consumed — but before `execute_job` returns on every path — add:

```rust
    if let Some(active) = registered_learning {
        let cron_invocation_id = active.invocation_id().to_owned();
        active.cleanup().await;
        // (cron_invocation_id captured for the anchor in Step 3)
    }
```

Because `execute_job` has multiple early returns, capture the id up front instead. Right after the registration block (Step 1), add:

```rust
    let cron_invocation_id = registered_learning
        .as_ref()
        .map(|a| a.invocation_id().to_owned());
```

and place the `cleanup().await` on the single terminal path of `execute_job` (the function runs to completion after the stream is consumed; there is no `?`-propagation — failures are logged and the function falls through). Add at the end of `execute_job`, after all delivery/learning work:

```rust
    if let Some(active) = registered_learning {
        active.cleanup().await;
    }
```

If any early `return` exists between registration and the end (e.g. the DB-insert failure at line 543 is BEFORE registration, so it's fine), ensure registration happens AFTER the last early return. The DB-insert guard at lines 540-546 returns before our block, so registration is safely after it. Verify no `return` occurs between Step 1's block and the end of `execute_job`; if one does, call `active.cleanup().await` (taking `registered_learning` by value via `.take()` on a `let mut`) before it. Use a `let mut registered_learning` so it can be moved out once.

- [ ] **Step 3: Stamp the anchor**

In the anchor construction (line 977-999) change the field added in Task 4:

```rust
                                        learning_invocation_id: cron_invocation_id.clone(),
```

- [ ] **Step 4: Unit-test the gating decision**

The full path needs a live sandbox+Claude; unit-test the pure decision that learning-enabled cron uses the keep-learning disallowed set. Add to `crates/bot/src/cron.rs` test module:

```rust
#[test]
fn cron_keeps_learning_tools_when_learning_enabled() {
    let with = crate::cc::invocation::disallow_foreground_only_tools_keep_learning(
        crate::cc::invocation::baseline_disallowed_tools(),
    );
    let without = crate::cc::invocation::disallow_foreground_only_tools(
        crate::cc::invocation::baseline_disallowed_tools(),
    );
    let start = right_mcp::internal_client::SKILL_LEARNING_START_MCP_TOOL;
    assert!(!with.iter().any(|t| t == start), "learning-enabled cron must allow skill_learning_start");
    assert!(without.iter().any(|t| t == start), "learning-disabled cron must disallow it");
}
```

- [ ] **Step 5: Add a live integration test (CI-gated)**

Per AGENTS.rust.md, a cron turn that actually runs `claude` inside the sandbox is `ci-claude`-ignored. Add to `crates/bot/tests/` (or the existing cron live-test module) a test named `ci_claude_cron_can_author_skill_inline`:

```rust
#[tokio::test]
#[ignore = "ci-claude: runs claude inside a sandbox to author a skill from a cron turn"]
async fn ci_claude_cron_can_author_skill_inline() {
    // Arrange: an agent with learning.prefilter_enabled = true and a recurring
    // cron whose prompt instructs a tiny verified procedure worth saving.
    // Act: trigger the cron once.
    // Assert: a rightx-* SKILL.md exists in the sandbox AND a
    // skill_learning_events finish row (status created/updated) exists for the
    // cron invocation AND created_by = 'cron' in skill_lifecycle AND the async
    // probe did NOT spawn a probe-writer for that run.
    // (Mirror the harness used by existing ci_claude_* cron tests.)
}
```

Fill the body using the existing `ci_claude_*` cron test harness in the repo as the template (read a sibling `ci_claude_` test first; do not invent a new harness).

- [ ] **Step 6: Run unit test — expect PASS**

Run: `devenv shell -- cargo nextest run -p bot cron_keeps_learning_tools_when_learning_enabled`
Expected: PASS. Confirm `devenv shell -- cargo build -p bot`.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/cron.rs crates/bot/tests/
git commit -m "feat(cron): allow inline skill authoring in cron turns"
```

---

## Task 7: Relax the `right-learn-skill` gate

**Files:**
- Modify: `crates/right-codegen/skills/right-learn-skill/SKILL.md:1-19` (frontmatter + intro)
- Modify: `crates/right-codegen/src/agent_def_tests.rs` (`right_learn_skill_prompt_uses_explicit_intent_framing`)
- Test: `crates/right-codegen` test suite

Component 2. Keep all required needles (protocol tools, `rightx-` prefix via `LEARNED_SKILL_PREFIX`, `.claude/skills/`, `source: "learned"`, the `send_progress` line, `LLM-authored receipt message`); never introduce `learning_signal`, `skill_issue_signal`, `rl-`, `_right-`.

- [ ] **Step 1: Update the breaking test FIRST (RED → defines the target)**

In `crates/right-codegen/src/agent_def_tests.rs`, replace the description assertion in `right_learn_skill_prompt_uses_explicit_intent_framing`:

```rust
    assert!(
        skill.contains("Use when you verified a reusable procedure this turn"),
        "right-learn-skill description should trigger on self-judgment"
    );
    assert!(
        !skill.contains("Use ONLY when the user explicitly asks"),
        "right-learn-skill must no longer hard-gate to explicit user intent only"
    );
```

Keep the other assertions in that test (the `mcp__right__skill_learning_start`/`_finish` and `LEARNED_SKILL_PREFIX` ones) unchanged.

- [ ] **Step 2: Run it — expect FAIL**

Run: `devenv shell -- cargo nextest run -p right-codegen right_learn_skill_prompt_uses_explicit_intent_framing`
Expected: FAIL — current SKILL.md still says "Use ONLY when the user explicitly asks".

- [ ] **Step 3: Rewrite the frontmatter description (lines 3-7)**

```yaml
description: >-
  Use when you verified a reusable procedure this turn (non-trivial multi-step
  work, concrete gotchas, or you just corrected a wrong approach) and want to
  save or fix a rightx-* skill, or when the user explicitly asks to save,
  remember, or fix one. Not for one-off tasks, unverified guesses, or trivial
  single-step work.
```

Bump `version: 0.2.0` → `version: 0.3.0`.

- [ ] **Step 4: Rewrite the intro (lines 12-19)**

Replace the "Use this skill ONLY when the user explicitly says …" paragraph with:

```markdown
# /right-learn-skill — Mid-Conversation Skill Writes

Create or fix a `rightx-*` learned skill, either:

- the user explicitly asks ("save this as a skill", "remember how to do X",
  "this skill is broken, fix it"), **or**
- on your own judgment, when *during this turn* you verified a reusable "how":
  non-trivial multi-step work, concrete gotchas / exact commands / API quirks,
  and you are confident it will recur. The strongest trigger is correcting a
  wrong approach and now knowing the right one.

The post-turn probe-writer is the safety net for turns where you did NOT
capture the "how" yourself; on any turn where you author or patch a skill, it
does not run. In a cron turn, only write a skill after your `delivery` output
is secured — the budget that produces the deliverable comes first.
```

Leave the "Skip", "Required Protocol", "Package Shape", and "Skill Quality" sections unchanged (they carry the required needles).

- [ ] **Step 5: Run the codegen test suite — expect PASS**

Run: `devenv shell -- cargo nextest run -p right-codegen right_learn_skill`
Expected: PASS for both `right_learn_skill_prompt_uses_explicit_intent_framing` and `right_learn_skill_mentions_protocol_and_boundaries` and `learned_skill_prompt_text_has_no_old_or_invalid_prefixes`.

- [ ] **Step 6: Commit**

```bash
git add crates/right-codegen/skills/right-learn-skill/SKILL.md crates/right-codegen/src/agent_def_tests.rs
git commit -m "feat(learning): allow self-judgment skill authoring in right-learn-skill"
```

---

## Task 8: `right-cron` — "what, not how"

**Files:**
- Modify: `crates/right-codegen/skills/right-cron/SKILL.md` (after the "Writing Cron Prompts" table, ~line 99)
- Test: `crates/right-codegen/src/skills.rs` (presence assertion)

Component 1.

- [ ] **Step 1: Write the failing presence test**

In `crates/right-codegen/src/skills.rs` test module (next to `right_learn_skill_mentions_protocol_and_boundaries`):

```rust
#[test]
fn right_cron_teaches_what_not_how() {
    let dir = tempdir().unwrap();
    install_builtin_skills(dir.path(), &MemoryProvider::File).unwrap();
    let content =
        std::fs::read_to_string(dir.path().join(".claude/skills/right-cron/SKILL.md")).unwrap();
    assert!(content.contains("What, not how"));
    assert!(content.contains("rightx-"));
}
```

- [ ] **Step 2: Run it — expect FAIL**

Run: `devenv shell -- cargo nextest run -p right-codegen right_cron_teaches_what_not_how`
Expected: FAIL.

- [ ] **Step 3: Add the subsection after line 99**

```markdown
### What, not how

A cron `prompt:` states the **goal** — the outcome you want — and trusts your
skills to supply the **procedure**. Do not inline brittle step-by-step "how"
into the prompt: it can't be improved centrally and rots as tools change. When
a cron's procedure is non-trivial, capture it as a `rightx-*` skill (see
right-learn-skill) before creating the cron, then write the cron's "what". At
fire time the cron loads the skill and executes.
```

- [ ] **Step 4: Run the test — expect PASS**

Run: `devenv shell -- cargo nextest run -p right-codegen right_cron_teaches_what_not_how`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/right-codegen/skills/right-cron/SKILL.md crates/right-codegen/src/skills.rs
git commit -m "feat(cron): teach what-not-how cron prompt authoring"
```

---

## Task 9: Update `docs/architecture/learning.md`

**Files:**
- Modify: `docs/architecture/learning.md`

Descriptive doc; cite-on-touch.

- [ ] **Step 1: Add an "Inline authoring" subsection**

After the "Per-turn pipeline" section, add:

```markdown
## Inline authoring (agent self-judgment)

Besides the async probe, the agent may author/patch a `rightx-*` skill mid-turn
via `right-learn-skill` and the `skill_learning_start`/`_finish` protocol, on
its own judgment (a verified reusable "how") or explicit user request. Available
in foreground (the `Foreground` invocation is learning-capable) and cron (a
learning-capable `Cron` invocation kind, registered per run when
`learning.prefilter_enabled`; non-message-sending — no live user).

On any turn where a skill was successfully created/updated, the async probe is
skipped entirely: `run_post_turn` returns early when
`ProbeAnchor.learning_invocation_id` has a successful `skill_learning_events`
finish row (`learning_pipeline::authored_skill_this_turn`). The probe is the
safety net for turns the agent did not self-capture.

Provenance: foreground inline writes → `created_by = foreground`; cron inline
writes → `created_by = cron` (`right_backend::invocation_kind_to_created_by`).
Both are now auto-managed by the curator (stale→archive when unused+unpinned),
alongside `probe_writer`/`curator`; the dashboard pin is the durability escape.
```

- [ ] **Step 2: Commit**

```bash
git add docs/architecture/learning.md
git commit -m "docs(learning): document inline authoring and probe-skip"
```

---

## Task 10: Update `PROMPT_SYSTEM.md`

**Files:**
- Modify: `PROMPT_SYSTEM.md`

- [ ] **Step 1: Sync the learning + cron prompt policy**

Find the section describing `right-learn-skill` (search `right-learn-skill`) and update it to state the relaxed trigger (self-judgment OR explicit user) and the probe-defer rule. Find the cron-prompt guidance (search `right-cron` / "Writing Cron Prompts") and add the "what, not how" principle. Keep edits factual and brief (operator-facing doc).

- [ ] **Step 2: Commit**

```bash
git add PROMPT_SYSTEM.md
git commit -m "docs: sync PROMPT_SYSTEM with inline authoring and cron what-not-how"
```

---

## Task 11: Add the `ARCHITECTURE.md` invariant

**Files:**
- Modify: `ARCHITECTURE.md` (Skill learning section)

Budget is tight: current size **39,029 bytes**, ceiling 40,000. The added line must be short.

- [ ] **Step 1: Add one rule line**

In the "Skill learning" section, after the paragraph about the per-turn pipeline call sites, add:

```markdown
The agent may author/patch `rightx-*` skills inline (foreground + cron via the
learning-capable `Cron` invocation kind); the async probe is skipped on any turn
that did so. Inline `foreground`/`cron` provenance skills are curator-auto-managed.
```

- [ ] **Step 2: Verify the budget**

Run: `wc -c ARCHITECTURE.md`
Expected: a number **< 40000**. If it exceeds, trim a redundant sentence elsewhere in the same section in the same commit (e.g. shorten the surrounding paragraph) until under budget.

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(arch): record inline-authoring probe-skip invariant"
```

---

## Task 12: Final workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Full workspace test**

Run: `devenv shell -- cargo nextest run --workspace`
Expected: PASS (ignored `ci_*` tests are not run locally; pre-existing flakes — cc/invocation pid race, dashboard warn-count — re-run isolated before blaming this change).

- [ ] **Step 2: Doctests**

Run: `devenv shell -- cargo test --doc --workspace`
Expected: PASS.

- [ ] **Step 3: Debug build**

Run: `devenv shell -- cargo build --workspace`
Expected: success.

- [ ] **Step 4: Final review pass**

Run the `rust-dev:review-rust-code` subagent over the diff; convert any real issues to follow-up fixes.

- [ ] **Step 5: Commit any review fixes, then the branch is ready**

```bash
git add -A && git commit -m "chore: address review for inline skill authoring"
```

---

## Self-Review

**Spec coverage:**
- Component 1 (cron what-not-how) → Task 8 (+ Task 10/11 docs).
- Component 2 (relax gate) → Task 7.
- Component 3 (probe-skip) → Task 4.
- Component 4 (cron inline authoring) → Tasks 3, 5, 6.
- Component 5 (provenance + lifecycle A) → Tasks 1, 2 (+ kind→provenance in Task 3).
- Component 6 (docs) → Tasks 9, 10, 11.
- Budget caution → folded into Task 7 skill text (kept off the every-cron-turn hot path, per token-budget reasoning).

**Type consistency:** `CreatedBy::Cron` ("cron") ↔ `ProgressInvocationKind::Cron` ↔ `ProgressInvocationKindDto::Cron` ("cron") ↔ `SkillCreatedBy::Cron`; `invocation_kind_to_created_by(Cron) = CreatedBy::Cron`. `learning_invocation_id: Option<String>` consistent across `ProbeAnchor`, `CcReply`, and `authored_skill_this_turn(conn, Option<&str>)`. Helper `disallow_foreground_only_tools_keep_learning` used by Task 6.

**Notable risks flagged for the implementer:**
- Task 6 cleanup must cover every `execute_job` exit path (use `let mut registered_learning` + `.take()`); `RegisteredNonForegroundInvocation` is `#[must_use]` and only `cleanup().await` unregisters.
- Task 4 touches 7 `CcReply` construction sites — all must set `learning_invocation_id` or the crate won't compile.
- Task 2 may break a sibling test that asserted foreground rows are never archived — that test encodes the OLD behavior and must be updated, not worked around.
