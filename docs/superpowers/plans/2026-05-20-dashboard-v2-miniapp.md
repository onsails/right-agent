# Dashboard v2 Miniapp Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Track progress by checking off each step.

**Goal:** Build the read-only Dashboard v2 Mini App with top-level `Overview`, `Activity`, `Knowledge`, `Usage`, `Identity`, and `Health`.

**Architecture:** Extend `right-dashboard` for DTOs and SQLite read models. Keep live sandbox/doctor probes in `right-bot::telegram::dashboard`. Preserve existing Mini App auth, agent allowlist checks, and agent-path boundaries on every route.

**Tech Stack:** Rust 2024, Axum, rusqlite, OpenShell gRPC helpers, teloxide, Vue 3, Vite, TypeScript, checked-in dashboard static assets.

**Source Spec:** `docs/superpowers/specs/2026-05-20-dashboard-v2-miniapp-design.md`

## Constraints

- Mini App is read-only. Refresh routes may read live state but must not mutate files, config, credentials, sandboxes, processes, or DB rows.
- `/doctor` remains a slash command and also becomes a Health view API.
- `/usage` is removed from Telegram command menus but manual `/usage` still opens the dashboard.
- Skills are Knowledge content, grouped into `core`, `learned`, and `other`.
- Identity is a top-level view.
- Sandbox process stats are sandbox-internal only.
- Overview must not implicitly run doctor; it shows `not_loaded` until Health is fetched.
- Final verification after all code work is mandatory: `devenv shell -- cargo test --workspace`.

## Task 1: Worktree And Baseline

**Read first:** `AGENTS.md`, `AGENTS.rust.md`, `ARCHITECTURE.md`, the source spec, and touched files under `docs/architecture/`.

- [ ] Create the implementation worktree from current `HEAD`, not `origin/master`, so the committed spec and this plan are present:

```bash
devenv shell -- git fetch origin master
devenv shell -- git worktree add .worktrees/dashboard-v2-miniapp -b feat/dashboard-v2-miniapp HEAD
cd .worktrees/dashboard-v2-miniapp
devenv shell -- git status --short --branch
```

- [ ] Before writing Rust, load `rust-dev:rust-dev` if it exists in the execution environment. If unavailable, state that once and follow `AGENTS.rust.md`.
- [ ] Run targeted baselines:

```bash
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo test -p right-bot dashboard::
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

- [ ] Record any pre-existing failures before editing.

## Task 2: Dashboard v2 DTO Contract

**Files:** `crates/right-dashboard/src/api_types.rs`, `crates/right-dashboard/frontend/src/types.ts`

- [ ] Add failing Rust serialization tests for:
  - `DashboardFeatures` with `activity`, `knowledge_learning`, `knowledge_skills`, `usage`, `identity`, `doctor`, `sandbox_stats`, and `commands_enabled: false`.
  - `DashboardOverviewResponse` with doctor `not_loaded` and sandbox `unknown`.
- [ ] Verify failures:

```bash
devenv shell -- cargo test -p right-dashboard dashboard_v2_bootstrap_features_serialize
devenv shell -- cargo test -p right-dashboard dashboard_overview_serializes_expected_shape
```

- [ ] Add Rust DTOs:
  - `DashboardOverviewResponse`
  - `OverviewDoctorStatus`
  - `OverviewSandboxStatus`
  - new `DashboardFeatures` flags above
- [ ] Mirror those types in `frontend/src/types.ts`.
- [ ] Update every existing `DashboardFeatures` construction in tests and bot code.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-dashboard dashboard_v2_bootstrap_features_serialize
devenv shell -- cargo test -p right-dashboard dashboard_overview_serializes_expected_shape
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/frontend/src/types.ts
devenv shell -- git commit -m "feat(dashboard): add v2 api types"
```

## Task 3: Activity Routes

**Files:** `crates/right-dashboard/src/read_model/activity.rs`, `crates/right-dashboard/src/read_model.rs`, `crates/bot/src/telegram/dashboard.rs`

- [ ] Add failing bot route tests:
  - `activity_overview_returns_current_cron_payload` for `/dashboard/{agent}/api/v1/activity/overview`.
  - `activity_run_detail_returns_not_found_for_unknown_run` for `/dashboard/{agent}/api/v1/activity/runs/{run_id}`.
- [ ] Verify failures with separate Cargo filters:

```bash
devenv shell -- cargo test -p right-bot activity_overview_returns_current_cron_payload
devenv shell -- cargo test -p right-bot activity_run_detail_returns_not_found_for_unknown_run
```

- [ ] Move current overview/run-detail SQL from `read_model.rs` into `read_model/activity.rs`.
- [ ] Public activity API:
  - `ActivityOverviewInput { agent, generated_at, refresh_interval_secs, foreground }`
  - `activity_overview(...) -> Result<OverviewResponse, ReadModelError>`
  - `activity_run_detail(...) -> Result<Option<RunDetailResponse>, ReadModelError>`
- [ ] Leave compatibility wrappers in `read_model.rs` for old `overview` and `run_detail`.
- [ ] Mount Activity routes and keep old `/api/v1/overview` and `/api/v1/runs/{run_id}` working until Dashboard overview replaces the old route.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-dashboard activity
devenv shell -- cargo test -p right-bot activity_
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/src/read_model.rs crates/right-dashboard/src/read_model/activity.rs crates/bot/src/telegram/dashboard.rs
devenv shell -- git commit -m "feat(dashboard): add activity routes"
```

## Task 4: Usage API

**Files:** `crates/right-dashboard/Cargo.toml`, `crates/right-dashboard/src/read_model/usage.rs`, `crates/right-dashboard/src/read_model.rs`, `crates/right-dashboard/src/api_types.rs`, `crates/right-dashboard/frontend/src/types.ts`, `crates/right-dashboard/frontend/src/api.ts`, `crates/bot/src/telegram/dashboard.rs`

- [ ] Add failing dashboard read-model test `usage_overview_builds_windows_and_sources`.
  - Insert interactive and cron `usage_events`.
  - Assert `today`, `last_7_days`, `last_30_days`, and `all_time` windows.
  - Assert per-source invocation/cost totals and per-model rows.
- [ ] Verify failure:

```bash
devenv shell -- cargo test -p right-dashboard usage_overview_builds_windows_and_sources
```

- [ ] Add `tracing = { workspace = true }` to `crates/right-dashboard/Cargo.toml`.
- [ ] Add usage DTOs:
  - `UsageOverviewResponse`
  - `UsageWindow`
  - `UsageSourceSummary`
  - `UsageModelSummary`
- [ ] Implement `read_model::usage::usage_overview(conn, UsageOverviewInput)` using the same fields as `right_agent::usage::aggregate`.
- [ ] Skip malformed `model_usage_json` rows with `tracing::warn!` instead of failing the whole response.
- [ ] Add bot route test `usage_returns_structured_windows_for_authorized_user`.
- [ ] Mount `GET /dashboard/{agent}/api/v1/usage`.
- [ ] Add `usageOverview()` in frontend `api.ts` and matching TypeScript types.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-dashboard usage_overview_builds_windows_and_sources
devenv shell -- cargo test -p right-bot usage_returns_structured_windows_for_authorized_user
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/Cargo.toml crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model.rs crates/right-dashboard/src/read_model/usage.rs crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/bot/src/telegram/dashboard.rs
devenv shell -- git commit -m "feat(dashboard): add usage api"
```

## Task 5: Learning Episodes API

**Files:** `crates/right-dashboard/src/read_model/learning_episodes.rs`, `crates/right-dashboard/src/read_model.rs`, `crates/right-dashboard/src/api_types.rs`, `crates/right-dashboard/frontend/src/types.ts`, `crates/right-dashboard/frontend/src/api.ts`, `crates/bot/src/telegram/dashboard.rs`

- [ ] Add failing read-model test `learning_episodes_list_links_reports`.
  - Insert a `learning_episodes` row and linked `skill_review_reports` row.
  - Assert episode summary includes linked report status.
- [ ] Verify failure:

```bash
devenv shell -- cargo test -p right-dashboard learning_episodes_list_links_reports
```

- [ ] Add DTOs:
  - `LearningEpisodesResponse`
  - `LearningEpisodeSummary`
  - `LearningEpisodeDetailResponse`
- [ ] Implement:
  - `learning_episodes(conn, LearningEpisodesInput)`
  - `learning_episode_detail(conn, agent, episode_id)`
  - join reports by `skill_review_reports.learning_episode_id`
  - parse JSON ref fields with `serde_json::from_str`
- [ ] Add bot test `learning_episodes_returns_data_for_authorized_user`.
- [ ] Mount:
  - `/api/v1/knowledge/learning/episodes`
  - `/api/v1/knowledge/learning/episodes/{episode_id}`
  - alias `/api/v1/knowledge/learning/reports/{report_id}` to the existing report handler
- [ ] Add frontend API functions `learningEpisodes()` and `learningEpisodeDetail(...)`.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-dashboard learning_episodes
devenv shell -- cargo test -p right-bot learning_episodes_returns_data_for_authorized_user
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model.rs crates/right-dashboard/src/read_model/learning_episodes.rs crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/bot/src/telegram/dashboard.rs
devenv shell -- git commit -m "feat(dashboard): expose learning episodes"
```

## Task 6: Skills Inventory API

**Files:** `crates/right-dashboard/src/skill_inventory.rs`, `crates/right-dashboard/src/lib.rs`, `crates/right-dashboard/src/api_types.rs`, `crates/right-dashboard/frontend/src/types.ts`, `crates/right-dashboard/frontend/src/api.ts`, `crates/bot/src/telegram/dashboard/skills.rs`, `crates/bot/src/telegram/dashboard.rs`

- [ ] Add failing dashboard test `scan_host_skills_groups_core_learned_and_other`.
  - Fixture path is `<agent_dir>/.claude/skills/{skill}/SKILL.md`.
  - `right_codegen::BUILTIN_SKILL_NAMES` or explicit core names classify `core`.
  - `rightx-*` classifies `learned`.
  - Everything else classifies `other`.
- [ ] Verify failure:

```bash
devenv shell -- cargo test -p right-dashboard scan_host_skills_groups_core_learned_and_other
```

- [ ] Add DTOs:
  - `SkillsResponse { agent, source, warning, groups }`
  - `SkillGroups { core, learned, other }`
  - `SkillSummary`
  - `SkillDetailResponse`
- [ ] Implement `scan_host_skills(agent, agent_dir, core_skill_names, source, preview_limit_bytes)`.
- [ ] Implement `read_host_skill_detail(agent_dir, skill_name, core_skill_names, preview_limit_bytes)`.
- [ ] Validate skill names: no `/`, `\`, NUL, or `..`.
- [ ] Parse bounded `description:` from `SKILL.md` frontmatter.
- [ ] Add bot `dashboard/skills.rs` with sandbox-first scan and host fallback. If sandbox scan fails, return host response with `warning`.
- [ ] Add bot test `skills_route_groups_host_skills_when_no_sandbox`.
- [ ] Mount:
  - `/api/v1/knowledge/skills`
  - `/api/v1/knowledge/skills/{skill_name}`
- [ ] Add frontend API functions `skillsOverview()` and `skillDetail(...)`.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-dashboard skill_inventory
devenv shell -- cargo test -p right-bot skills_route_groups_host_skills_when_no_sandbox
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/lib.rs crates/right-dashboard/src/skill_inventory.rs crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/dashboard/skills.rs
devenv shell -- git commit -m "feat(dashboard): add skills inventory"
```

## Task 7: Identity API

**Files:** `crates/right-dashboard/src/identity_files.rs`, `crates/right-dashboard/src/lib.rs`, `crates/right-dashboard/src/api_types.rs`, `crates/right-dashboard/frontend/src/types.ts`, `crates/right-dashboard/frontend/src/api.ts`, `crates/bot/src/telegram/dashboard/identity.rs`, `crates/bot/src/telegram/dashboard.rs`

- [ ] Add failing tests:
  - `read_host_identity_files_reports_sources_per_file`
  - `validate_identity_file_rejects_path_traversal`
- [ ] Verify failure:

```bash
devenv shell -- cargo test -p right-dashboard identity_files
```

- [ ] Add DTOs:
  - `IdentityResponse`
  - `IdentityFileSummary`
- [ ] Implement host reader for exactly `IDENTITY.md`, `SOUL.md`, `USER.md`.
- [ ] Cap each file to 64 KiB and mark truncation.
- [ ] Add bot `dashboard/identity.rs` with sandbox-first bounded reads and host fallback. Use `if let Some(sandbox)` rather than unwrapping `resolved_sandbox`.
- [ ] Add bot test `identity_route_returns_host_files_without_sandbox`.
- [ ] Mount `/api/v1/identity`.
- [ ] Add frontend API function `identityFiles()`.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-dashboard identity_files
devenv shell -- cargo test -p right-bot identity_route_returns_host_files_without_sandbox
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/lib.rs crates/right-dashboard/src/identity_files.rs crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/dashboard/identity.rs
devenv shell -- git commit -m "feat(dashboard): add identity view api"
```

## Task 8: Health API

**Files:** `crates/right-dashboard/src/api_types.rs`, `crates/right-dashboard/frontend/src/types.ts`, `crates/right-dashboard/frontend/src/api.ts`, `crates/bot/src/telegram/dashboard/health.rs`, `crates/bot/src/telegram/dashboard.rs`

- [ ] Add failing health helper tests:
  - `parse_process_lines_bounds_process_count`
  - `doctor_response_groups_statuses`
- [ ] Verify failures:

```bash
devenv shell -- cargo test -p right-bot parse_process_lines_bounds_process_count
devenv shell -- cargo test -p right-bot doctor_response_groups_statuses
```

- [ ] Add DTOs:
  - `DoctorResponse`
  - `DoctorCheckResponse`
  - `SandboxStatsResponse`
  - `SandboxDiskStats`
  - `SandboxMemoryStats`
  - `SandboxProcess`
- [ ] Implement helpers in `dashboard/health.rs`:
  - `doctor_response_from_checks(agent, checks)`
  - `parse_ps_output(stdout, limit)`
  - `sandbox_stats_response(agent, resolved_sandbox)`
- [ ] Sandbox stats read disk, memory, and up to 50 processes. Convert RSS KiB to bytes and cap command strings to 160 chars.
- [ ] Add route tests:
  - `doctor_route_returns_grouped_checks_for_authorized_user`
  - `sandbox_route_without_sandbox_returns_unavailable_snapshot`
- [ ] Mount:
  - `/api/v1/health/doctor`
  - `/api/v1/health/sandbox`
- [ ] `handle_health_doctor` runs `right_agent::doctor::run_doctor(&state.home)`.
- [ ] Add frontend API functions `doctorStatus()` and `sandboxStats()`.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-bot health::
devenv shell -- cargo test -p right-bot doctor_route_returns_grouped_checks_for_authorized_user
devenv shell -- cargo test -p right-bot sandbox_route_without_sandbox_returns_unavailable_snapshot
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/frontend/src/types.ts crates/right-dashboard/frontend/src/api.ts crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/dashboard/health.rs
devenv shell -- git commit -m "feat(dashboard): add health api"
```

## Task 9: Dashboard State, Bootstrap, And Overview API

**Files:** `crates/right-dashboard/src/read_model/dashboard_overview.rs`, `crates/right-dashboard/src/read_model.rs`, `crates/bot/src/telegram/dashboard.rs`, `crates/bot/src/lib.rs`, `crates/right-dashboard/frontend/src/api.ts`

- [ ] Add failing bootstrap assertions for every v2 feature flag and `commands_enabled: false`.
- [ ] Add failing read-model test `dashboard_overview_summarizes_activity_usage_and_learning`.
  - Insert a running `async_runs` row with required `target_chat_id`.
  - Insert a `usage_events` row.
  - Insert a candidate `skill_review_reports` row.
  - Assert active run count includes active foreground count, today cost, and learning candidates.
- [ ] Verify failures:

```bash
devenv shell -- cargo test -p right-bot bootstrap_exposes_learning_capabilities
devenv shell -- cargo test -p right-dashboard dashboard_overview_summarizes_activity_usage_and_learning
```

- [ ] Extend `DashboardState` with `home: PathBuf` and `resolved_sandbox: Option<String>`.
- [ ] In `crates/bot/src/lib.rs`, compute `resolved_sandbox` before `build_dashboard_router` and pass both new fields.
- [ ] Fill all v2 bootstrap flags.
- [ ] Implement `read_model::dashboard_overview`.
- [ ] Change `/api/v1/overview` to return `DashboardOverviewResponse`.
- [ ] Keep Activity data under `/api/v1/activity/overview`.
- [ ] Add frontend `dashboardOverview()` request for `/api/v1/overview`.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-dashboard dashboard_overview_summarizes_activity_usage_and_learning
devenv shell -- cargo test -p right-bot bootstrap_exposes_learning_capabilities
devenv shell -- cargo test -p right-bot overview_returns_data_for_authorized_user
devenv shell -- cargo test -p right-bot dashboard::
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/src/read_model.rs crates/right-dashboard/src/read_model/dashboard_overview.rs crates/bot/src/telegram/dashboard.rs crates/bot/src/lib.rs crates/right-dashboard/frontend/src/api.ts
devenv shell -- git commit -m "feat(dashboard): wire v2 overview state"
```

## Task 10: Manual /usage Compatibility

**Files:** `crates/bot/src/telegram/dispatch.rs`, `crates/bot/src/telegram/handler.rs`

- [ ] Add failing test `visible_commands_hide_usage_but_keep_dashboard`.
- [ ] Verify failure:

```bash
devenv shell -- cargo test -p right-bot visible_commands_hide_usage_but_keep_dashboard
```

- [ ] Add helper `visible_bot_commands()` that filters `usage` from `BotCommand::bot_commands()`.
- [ ] Use the helper for Telegram command registration.
- [ ] Keep `BotCommand::Usage(String)` and the dispatch branch so manual `/usage` still parses.
- [ ] Change `handle_usage` to delegate to `handle_dashboard(bot, msg, home, agent_dir, allowlist).await`.
- [ ] Update dptree dependencies if the signature needs `RightHome` and `AllowlistHandle`.
- [ ] Remove only imports made unused by this change.
- [ ] Verify:

```bash
devenv shell -- cargo test -p right-bot visible_commands_hide_usage_but_keep_dashboard
devenv shell -- cargo test -p right-bot dashboard_url_
devenv shell -- cargo check -p right-bot
```

- [ ] Commit:

```bash
devenv shell -- git add crates/bot/src/telegram/dispatch.rs crates/bot/src/telegram/handler.rs
devenv shell -- git commit -m "feat(bot): move usage command to dashboard"
```

## Task 11: Frontend Shell And Views

**Files:** `crates/right-dashboard/frontend/src/App.vue`, `crates/right-dashboard/frontend/src/api.ts`, `crates/right-dashboard/frontend/src/types.ts`, new files under `crates/right-dashboard/frontend/src/components/` and `crates/right-dashboard/frontend/src/views/`

- [ ] Create shared components:
  - `components/AppShell.vue`
  - `components/MetricCard.vue`
  - `components/StatusPill.vue`
- [ ] Split views:
  - `views/OverviewView.vue`
  - `views/ActivityView.vue`
  - `views/KnowledgeView.vue`
  - `views/UsageView.vue`
  - `views/IdentityView.vue`
  - `views/HealthView.vue`
  - `views/learning/EpisodesView.vue`
  - `views/learning/ReportsView.vue`
  - `views/SkillsView.vue`
- [ ] Keep Telegram init, bootstrap loading, connection state, active tab, and API orchestration in `App.vue`.
- [ ] Move current cron/run UI into `ActivityView.vue`.
- [ ] Implement `OverviewView` with MetricCards for active runs, failures, today cost, learning candidates, doctor state, and sandbox state.
- [ ] Implement `KnowledgeView` with subviews `episodes`, `reports`, and `skills`.
- [ ] Implement `UsageView` as compact source/model tables.
- [ ] Implement `IdentityView` as segmented `IDENTITY.md`, `SOUL.md`, `USER.md` viewer with source/truncation/warning states.
- [ ] Implement `HealthView` with explicit refresh buttons for doctor and sandbox snapshots.
- [ ] Do not use nested cards. Keep dense operational layout and stable mobile dimensions.
- [ ] Verify:

```bash
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/frontend/src
devenv shell -- git commit -m "feat(dashboard): build v2 miniapp views"
```

## Task 12: Static Assets And Architecture Docs

**Files:** `crates/right-dashboard/static/dashboard/`, `ARCHITECTURE.md` if drifted, `docs/architecture/modules.md`, `docs/architecture/lifecycle.md`, `docs/architecture/sandbox.md`, `docs/architecture/sessions.md` if run/session semantics changed

- [ ] Rebuild checked-in dashboard assets:

```bash
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

- [ ] Update `docs/architecture/modules.md` for new dashboard read-model modules and helpers:
  - `read_model/activity.rs`
  - `read_model/usage.rs`
  - `read_model/learning_episodes.rs`
  - `read_model/dashboard_overview.rs`
  - `skill_inventory.rs`
  - `identity_files.rs`
- [ ] Update `docs/architecture/lifecycle.md` dashboard bullet to mention read-only v1 API domains: overview, activity, knowledge, usage, identity, health.
- [ ] Update `docs/architecture/sandbox.md` to mention bounded read-only Health probes for disk, memory, and processes.
- [ ] Update `ARCHITECTURE.md` only if route ownership or crate boundaries changed.
- [ ] Verify staged scope:

```bash
devenv shell -- git status --short
devenv shell -- git diff --stat
```

- [ ] Commit:

```bash
devenv shell -- git add crates/right-dashboard/static/dashboard docs/architecture/modules.md docs/architecture/lifecycle.md docs/architecture/sandbox.md
devenv shell -- git add ARCHITECTURE.md docs/architecture/sessions.md
devenv shell -- git commit -m "docs(dashboard): document v2 miniapp"
```

If `ARCHITECTURE.md` or `docs/architecture/sessions.md` did not change, omit
them from `git add`.

## Task 13: Final Verification

- [ ] Run full workspace tests:

```bash
devenv shell -- cargo test --workspace
```

- [ ] Run full workspace build:

```bash
devenv shell -- cargo build --workspace
```

- [ ] Run frontend checks:

```bash
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

- [ ] Check final branch state:

```bash
devenv shell -- git status --short
devenv shell -- git log --oneline -n 12
```

- [ ] Final implementation response must include the exact verification commands and whether they passed. Do not claim completion if any command fails.
