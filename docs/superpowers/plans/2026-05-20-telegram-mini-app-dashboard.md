# Telegram Mini App Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a read-only, cron-centered Telegram Mini App dashboard for one Right Agent.

**Architecture:** Add a `right-dashboard` library crate for auth validation, DTOs, read models, and static assets. `right-bot` mounts bot-owned Axum routes on the existing per-agent UDS server, injects bot token/allowlist/runtime state, exposes `/dashboard`, and sets the Telegram menu button. Vue assets are built with Vite and checked in under the dashboard crate so Cargo builds do not require npm or network.

**Tech Stack:** Rust 2024, Axum 0.8, Teloxide 0.17, Rusqlite, HMAC-SHA256, Vue 3, Vite, TypeScript, npm, cloudflared.

---

## Preconditions

- Use a clean worktree or create one under `.worktrees/` before implementation.
- Before writing Rust, load `rust-dev:rust-dev` if available; otherwise state that it is unavailable and follow `AGENTS.rust.md`.
- Re-read `docs/superpowers/specs/2026-05-20-telegram-mini-app-dashboard-design.md`, `ARCHITECTURE.md`, `docs/architecture/modules.md`, `docs/architecture/lifecycle.md`, and `docs/architecture/sessions.md`.
- Use `devenv shell -- ...` for commands.
- Do not commit `.superpowers/` visual companion files or `node_modules/`.

## File Map

Create:

- `crates/right-dashboard/Cargo.toml`
- `crates/right-dashboard/src/lib.rs`
- `crates/right-dashboard/src/auth.rs`
- `crates/right-dashboard/src/api_types.rs`
- `crates/right-dashboard/src/read_model.rs`
- `crates/right-dashboard/src/assets.rs`
- `crates/right-dashboard/frontend/package.json`
- `crates/right-dashboard/frontend/package-lock.json`
- `crates/right-dashboard/frontend/index.html`
- `crates/right-dashboard/frontend/tsconfig.json`
- `crates/right-dashboard/frontend/vite.config.ts`
- `crates/right-dashboard/frontend/src/main.ts`
- `crates/right-dashboard/frontend/src/App.vue`
- `crates/right-dashboard/frontend/src/api.ts`
- `crates/right-dashboard/frontend/src/types.ts`
- `crates/right-dashboard/static/dashboard/index.html` and generated Vite assets
- `crates/bot/src/telegram/dashboard.rs`

Modify:

- `Cargo.toml`
- `devenv.nix`
- `crates/bot/Cargo.toml`
- `crates/bot/src/telegram/mod.rs`
- `crates/bot/src/telegram/oauth_callback.rs`
- `crates/bot/src/telegram/dispatch.rs`
- `crates/bot/src/telegram/handler.rs`
- `crates/bot/src/lib.rs`
- `crates/right-codegen/templates/cloudflared-config.yml.j2`
- `crates/right-codegen/src/cloudflared.rs`
- `crates/right-codegen/src/cloudflared_tests.rs`
- `ARCHITECTURE.md`
- `docs/architecture/modules.md`
- `docs/architecture/lifecycle.md`

Do not modify `PROMPT_SYSTEM.md`.

## Task 0: Baseline

**Files:**
- Inspect only.

- [ ] **Step 1: Check worktree**

Run:

```bash
devenv shell -- git status --short
```

Expected: no tracked modifications. Untracked `.superpowers/` is acceptable.

- [ ] **Step 2: Run focused baseline**

Run:

```bash
devenv shell -- cargo test -p right-codegen cloudflared
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic
```

Expected: both pass. Record any pre-existing failure before editing.

## Task 1: Scaffold `right-dashboard`

**Files:**
- Modify: `Cargo.toml`
- Modify: `devenv.nix`
- Modify: `crates/bot/Cargo.toml`
- Create: `crates/right-dashboard/Cargo.toml`
- Create: `crates/right-dashboard/src/lib.rs`
- Create: `crates/right-dashboard/src/auth.rs`
- Create: `crates/right-dashboard/src/api_types.rs`
- Create: `crates/right-dashboard/src/read_model.rs`
- Create: `crates/right-dashboard/src/assets.rs`
- Create: `crates/right-dashboard/static/dashboard/index.html`

- [ ] **Step 1: Add workspace member and tooling**

In root `Cargo.toml`, add:

```toml
"crates/right-dashboard",
```

In `devenv.nix`, add `nodejs` to `packages`.

In `crates/bot/Cargo.toml`, add:

```toml
right-dashboard = { path = "../right-dashboard", version = "*" }
hmac = { workspace = true }
```

- [ ] **Step 2: Create crate manifest**

Create `crates/right-dashboard/Cargo.toml`:

```toml
[package]
name = "right-dashboard"
version.workspace = true
edition.workspace = true
publish = false

[dependencies]
chrono = { workspace = true }
hmac = { workspace = true }
include_dir = { workspace = true }
rusqlite = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
sha2 = { workspace = true }
subtle = { workspace = true }
thiserror = { workspace = true }
url = { workspace = true }

[dev-dependencies]
right-db = { path = "../right-db", version = "*" }
tempfile = { workspace = true }
```

- [ ] **Step 3: Create module shells**

Create `crates/right-dashboard/src/lib.rs`:

```rust
pub mod api_types;
pub mod assets;
pub mod auth;
pub mod read_model;
```

Create `crates/right-dashboard/src/auth.rs` with these public items:

```rust
DashboardUser { id, username, first_name }
InitDataValidation { bot_token, now, max_age_secs }
AuthError::{MissingInitData, MalformedInitData, InvalidHash, Expired, MissingUser, UnauthorizedUser}
validate_init_data(raw, cfg) -> Result<DashboardUser, AuthError>
authorize_user(user, trusted_user_ids: &BTreeSet<i64>) -> Result<DashboardUser, AuthError>
```

Create `crates/right-dashboard/src/api_types.rs` with `ApiErrorBody { error, detail }`.

Create `crates/right-dashboard/src/read_model.rs`:

```rust
use rusqlite::Connection;

pub fn smoke_read_model(_conn: &Connection) -> usize {
    0
}
```

Create `crates/right-dashboard/src/assets.rs` with `include_dir!("$CARGO_MANIFEST_DIR/static/dashboard")`, an `asset(path)` lookup, and content types for `.html`, `.js`, `.css`, `.svg`, and fallback `application/octet-stream`.

Create `crates/right-dashboard/static/dashboard/index.html` with a minimal dashboard shell.

- [ ] **Step 4: Verify scaffold**

Run:

```bash
devenv shell -- cargo test -p right-dashboard
```

Expected: pass.

- [ ] **Step 5: Commit**

Run:

```bash
devenv shell -- git add Cargo.toml devenv.nix crates/bot/Cargo.toml crates/right-dashboard
devenv shell -- git commit -m "feat(dashboard): scaffold mini app crate"
```

## Task 2: Implement Telegram Mini App Auth

**Files:**
- Modify: `crates/right-dashboard/src/auth.rs`

- [ ] **Step 1: Write failing tests**

Add tests in `crates/right-dashboard/src/auth.rs` for:

```rust
#[test]
fn valid_init_data_returns_user() { /* signed user id 42 returns DashboardUser */ }

#[test]
fn invalid_hash_is_rejected() { /* tampered hash returns AuthError::InvalidHash */ }

#[test]
fn expired_auth_date_is_rejected() { /* old auth_date returns AuthError::Expired */ }

#[test]
fn missing_user_is_rejected() { /* signed payload without user returns AuthError::MissingUser */ }

#[test]
fn authorize_user_requires_allowlist_membership() { /* user 42 not in BTreeSet returns UnauthorizedUser */ }
```

Use this signing pattern inside the test module:

```rust
let secret_key = Hmac::<Sha256>::new_from_slice(b"WebAppData")
    .unwrap()
    .chain_update(bot_token.as_bytes())
    .finalize()
    .into_bytes();
let hash = Hmac::<Sha256>::new_from_slice(&secret_key)
    .unwrap()
    .chain_update(data_check_string.as_bytes())
    .finalize()
    .into_bytes();
```

- [ ] **Step 2: Verify red**

Run:

```bash
devenv shell -- cargo test -p right-dashboard auth
```

Expected: auth validation tests fail while `validate_init_data` is still a stub.

- [ ] **Step 3: Implement validation**

Implement `validate_init_data` to:

- parse `url::form_urlencoded::parse(raw.as_bytes())`;
- require `hash`;
- sort all pairs except `hash` and `signature`;
- build Telegram `data_check_string` with `key=value` lines joined by `\n`;
- derive `secret_key = HMAC_SHA256("WebAppData", bot_token)`;
- compute expected hex HMAC over `data_check_string`;
- compare expected and supplied hash with `subtle::ConstantTimeEq`;
- reject negative or too-old age from `auth_date`;
- deserialize `user` JSON with `id`, `username`, and `first_name`;
- return `DashboardUser`.

Use `format!("{b:02x}")` for hex and never log raw init data.

- [ ] **Step 4: Verify green**

Run:

```bash
devenv shell -- cargo test -p right-dashboard auth
```

Expected: pass.

- [ ] **Step 5: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/src/auth.rs
devenv shell -- git commit -m "feat(dashboard): validate telegram mini app auth"
```

## Task 3: Add DTOs And Read Models

**Files:**
- Modify: `crates/right-dashboard/src/api_types.rs`
- Modify: `crates/right-dashboard/src/read_model.rs`

- [ ] **Step 1: Define DTOs**

In `api_types.rs`, add serializable DTOs:

```rust
pub struct BootstrapResponse { pub agent: String, pub api_version: String, pub refresh_interval_secs: u64, pub user_id: i64, pub features: DashboardFeatures }
pub struct DashboardFeatures { pub readonly: bool, pub commands_enabled: bool }
pub struct OverviewResponse { pub agent: String, pub generated_at: String, pub refresh_interval_secs: u64, pub summary: OverviewSummary, pub crons: Vec<CronCard>, pub active: ActiveActivity }
pub struct OverviewSummary { pub cron_count: usize, pub active_cron_count: usize, pub failed_recent_cron_count: usize, pub today_cost_usd: f64 }
pub struct CronCard { pub job_name: String, pub schedule: String, pub recurring: bool, pub run_at: Option<String>, pub target_chat_id: Option<i64>, pub target_thread_id: Option<i64>, pub max_budget_usd: f64, pub last_run: Option<RunSummary>, pub recent_runs: Vec<RunSummary> }
pub struct ActiveActivity { pub foreground: Vec<ForegroundActivity>, pub background: Vec<RunSummary> }
pub struct ForegroundActivity { pub chat_id: i64, pub thread_id: i64, pub turn_id: u64 }
pub struct RunSummary { pub id: String, pub kind: String, pub producer_ref: Option<String>, pub status: String, pub started_at: Option<String>, pub finished_at: Option<String>, pub exit_code: Option<i64>, pub delivery_status: String, pub cost_usd: Option<f64> }
pub struct RunDetailResponse { pub run: RunSummary, pub summary: Option<String>, pub notify_json: Option<serde_json::Value>, pub no_notify_reason: Option<String>, pub log: LogExcerpt }
pub struct LogExcerpt { pub available: bool, pub path: Option<String>, pub lines: Vec<String>, pub truncated: bool }
```

Each struct must derive `Debug`, `Clone`, `Serialize`, `Deserialize`, and `PartialEq`.

- [ ] **Step 2: Write failing read-model tests**

In `read_model.rs`, write tests with a temp `right_db::open_connection(dir, true)` fixture that inserts:

- one `cron_specs` row named `daily`;
- one completed `async_runs` row for `daily`;
- one running `background` row;
- one `usage_events` row for session `run-1`.

Assert:

```rust
assert_eq!(response.summary.cron_count, 1);
assert_eq!(response.summary.today_cost_usd, 0.25);
assert_eq!(response.crons[0].last_run.as_ref().unwrap().id, "run-1");
assert_eq!(response.active.background[0].id, "bg-1");
```

Also test:

```rust
assert!(run_detail(&conn, "missing", 20).unwrap().is_none());
assert!(!run_detail(&conn, "run-1", 20).unwrap().unwrap().log.available);
```

- [ ] **Step 3: Verify red**

Run:

```bash
devenv shell -- cargo test -p right-dashboard overview run_detail
```

Expected: fail because `overview` and `run_detail` are not implemented.

- [ ] **Step 4: Implement `overview`**

Implement:

```rust
pub struct OverviewInput {
    pub agent: String,
    pub generated_at: String,
    pub refresh_interval_secs: u64,
    pub foreground: Vec<ForegroundActivity>,
}

pub fn overview(conn: &Connection, input: OverviewInput) -> rusqlite::Result<OverviewResponse>
```

SQL requirements:

- read `cron_specs` ordered by `job_name`;
- for each cron, read up to 5 `async_runs` rows where `kind = 'cron'` and `producer_ref = job_name`;
- left join `usage_events` on `usage_events.session_uuid = async_runs.run_session_id`;
- read active background rows where `kind = 'background'` and `status IN ('queued', 'running')`;
- compute today's cost by summing `usage_events.total_cost_usd` for `ts >= <generated_at date>T00:00:00Z`.

- [ ] **Step 5: Implement `run_detail`**

Implement:

```rust
pub fn run_detail(conn: &Connection, run_id: &str, max_lines: usize) -> rusqlite::Result<Option<RunDetailResponse>>
```

Behavior:

- return `Ok(None)` for unknown run id;
- return metadata from `async_runs`;
- parse `notify_json` to `serde_json::Value` when valid;
- left join completed cost from `usage_events`;
- read last `max_lines` from `log_path` when the file exists;
- return `LogExcerpt { available: false, path: Some(path), lines: vec![], truncated: false }` for missing log files.

- [ ] **Step 6: Verify green**

Run:

```bash
devenv shell -- cargo test -p right-dashboard
```

Expected: pass.

- [ ] **Step 7: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/src/api_types.rs crates/right-dashboard/src/read_model.rs
devenv shell -- git commit -m "feat(dashboard): add read models"
```

## Task 4: Mount Dashboard Routes In Bot

**Files:**
- Create: `crates/bot/src/telegram/dashboard.rs`
- Modify: `crates/bot/src/telegram/mod.rs`
- Modify: `crates/bot/src/telegram/oauth_callback.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`

- [ ] **Step 1: Write route tests first**

In `dashboard.rs`, add tests for:

```rust
#[tokio::test]
async fn static_index_loads_without_auth() { /* GET /dashboard/alpha/ returns 200 */ }

#[tokio::test]
async fn api_rejects_missing_auth() { /* GET /dashboard/alpha/api/v1/bootstrap returns 401 */ }

#[tokio::test]
async fn api_rejects_agent_mismatch() { /* GET /dashboard/beta/api/v1/bootstrap returns 403 */ }

#[tokio::test]
async fn overview_returns_data_for_authorized_user() { /* signed allowlisted user returns 200 */ }

#[tokio::test]
async fn run_detail_returns_not_found_for_unknown_run() { /* signed allowlisted user + missing run returns 404 */ }
```

Use `AllowlistHandle::new(AllowlistState::from_file(...))` with one `AllowedUser { id: 42, label: None, added_by: None, added_at: parsed_utc }`. Sign test init data with the same HMAC algorithm as Task 2.

- [ ] **Step 2: Verify red**

Run:

```bash
devenv shell -- cargo test -p right-bot dashboard::
```

Expected: fail because the dashboard router does not exist.

- [ ] **Step 3: Implement dashboard router**

Create `DashboardState`:

```rust
#[derive(Clone)]
pub(crate) struct DashboardState {
    pub agent_name: String,
    pub bot_token: String,
    pub agent_dir: std::path::PathBuf,
    pub allowlist: right_agent::agent::allowlist::AllowlistHandle,
    pub foreground: super::StopTokens,
}
```

Implement `build_dashboard_router(state)` with routes:

```rust
GET /dashboard/{agent}/
GET /dashboard/{agent}/{*asset}
GET /dashboard/{agent}/api/v1/bootstrap
GET /dashboard/{agent}/api/v1/overview
GET /dashboard/{agent}/api/v1/runs/{run_id}
```

Rules:

- static assets only require agent path match;
- API routes require `Authorization: tma <raw-init-data>`;
- invalid/missing/expired auth returns `401`;
- valid Telegram auth from a non-trusted user returns `403`;
- agent path mismatch returns `403`;
- `overview` passes active foreground turns from `StopTokens`;
- DB open/query errors log and return `500` with `ApiErrorBody`;
- unknown run id returns `404`.

- [ ] **Step 4: Mount router in the UDS app**

In `telegram/mod.rs`, add:

```rust
pub(crate) mod dashboard;
```

In `oauth_callback.rs`, add a `dashboard_router: Router` parameter to `build_router` and `run_bot_uds_server`, then `.merge(dashboard_router)` before the Telegram webhook `.nest(...)`.

In `lib.rs`, add `use dashmap::DashMap;`, create:

```rust
let dashboard_foreground: telegram::StopTokens = Arc::new(DashMap::new());
let dashboard_router = telegram::dashboard::build_dashboard_router(telegram::dashboard::DashboardState {
    agent_name: args.agent.clone(),
    bot_token: token.clone(),
    agent_dir: agent_dir.clone(),
    allowlist: allowlist.clone(),
    foreground: dashboard_foreground.clone(),
});
```

Pass `dashboard_router` to `run_bot_uds_server`.

In `dispatch.rs`, add `stop_tokens: super::StopTokens` to `run_telegram` before `progress_state`, remove its local `StopTokens` creation, and pass `Arc::clone(&dashboard_foreground)` from `lib.rs`.

- [ ] **Step 5: Verify green**

Run:

```bash
devenv shell -- cargo test -p right-bot dashboard::
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic
```

Expected: pass.

- [ ] **Step 6: Commit**

Run:

```bash
devenv shell -- git add crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/mod.rs crates/bot/src/telegram/oauth_callback.rs crates/bot/src/lib.rs crates/bot/src/telegram/dispatch.rs
devenv shell -- git commit -m "feat(bot): expose dashboard routes"
```

## Task 5: Add Telegram Launch Surface And Cloudflared Route

**Files:**
- Modify: `crates/bot/src/telegram/dashboard.rs`
- Modify: `crates/bot/src/telegram/handler.rs`
- Modify: `crates/bot/src/telegram/dispatch.rs`
- Modify: `crates/bot/src/lib.rs`
- Modify: `crates/right-codegen/templates/cloudflared-config.yml.j2`
- Modify: `crates/right-codegen/src/cloudflared.rs`
- Modify: `crates/right-codegen/src/cloudflared_tests.rs`

- [ ] **Step 1: Add failing cloudflared test**

In `cloudflared_tests.rs`, add:

```rust
#[test]
fn dashboard_ingress_rule_per_agent() {
    let creds = fixture_creds();
    let agents = vec![("alpha".to_string(), PathBuf::from("/tmp/agents/alpha"))];
    let yaml = generate_cloudflared_config(&agents, "t.example.com", &creds).unwrap();
    assert!(yaml.contains("path: /dashboard/alpha/.*"), "missing dashboard ingress: {yaml}");
}
```

Run:

```bash
devenv shell -- cargo test -p right-codegen dashboard_ingress_rule_per_agent
```

Expected: fail.

- [ ] **Step 2: Add ingress**

In `cloudflared-config.yml.j2`, add after `/tg`:

```jinja
  - hostname: {{ tunnel_hostname }}
    path: /dashboard/{{ agent.name }}/.*
    service: unix:{{ agent.socket_path }}
```

Update `cloudflared.rs` docs to mention `/dashboard`.

- [ ] **Step 3: Add `/dashboard` command**

In `dashboard.rs`, add:

```rust
pub(crate) fn dashboard_url(hostname: &str, agent_name: &str) -> Result<url::Url, url::ParseError> {
    url::Url::parse(&format!(
        "https://{}/dashboard/{}/",
        hostname.trim_end_matches('/').trim_start_matches("https://").trim_start_matches("http://"),
        agent_name
    ))
}
```

In `handler.rs`, add `handle_dashboard` that reads global config, builds `InlineKeyboardButton::web_app("Open dashboard", WebAppInfo { url })`, and sends a `Dashboard` message. Use the existing private `to_request_err` helper in `handler.rs`.

In `dispatch.rs`, add:

```rust
#[command(description = "Open dashboard")]
Dashboard,
```

Import `handle_dashboard` from `handler.rs` and route:

```rust
.branch(dptree::case![BotCommand::Dashboard].endpoint(handle_dashboard))
```

- [ ] **Step 4: Set persistent menu button**

In `lib.rs`, after `global_cfg` is available, spawn a best-effort task:

```rust
let menu_bot = telegram::bot::build_bot(token.clone());
let menu_hostname = global_cfg.tunnel.hostname.clone();
let menu_agent = args.agent.clone();
tokio::spawn(async move {
    use teloxide::prelude::Requester as _;
    use teloxide::types::{MenuButton, WebAppInfo};

    match telegram::dashboard::dashboard_url(&menu_hostname, &menu_agent) {
        Ok(url) => {
            if let Err(e) = menu_bot
                .set_chat_menu_button()
                .menu_button(MenuButton::WebApp {
                    text: "Dashboard".to_string(),
                    web_app: WebAppInfo { url },
                })
                .await
            {
                tracing::warn!("set_chat_menu_button failed: {e:#}");
            }
        }
        Err(e) => tracing::warn!("dashboard menu URL invalid: {e:#}"),
    }
});
```

- [ ] **Step 5: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-codegen cloudflared
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic
devenv shell -- cargo check -p right-bot
```

Expected: pass.

Commit:

```bash
devenv shell -- git add crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/handler.rs crates/bot/src/telegram/dispatch.rs crates/bot/src/lib.rs crates/right-codegen/templates/cloudflared-config.yml.j2 crates/right-codegen/src/cloudflared.rs crates/right-codegen/src/cloudflared_tests.rs
devenv shell -- git commit -m "feat(dashboard): add telegram launch surface"
```

## Task 6: Build Vue Frontend

**Files:**
- Create/modify: `crates/right-dashboard/frontend/**`
- Modify: `crates/right-dashboard/static/dashboard/**`

- [ ] **Step 1: Create frontend package**

Create `package.json` with scripts:

```json
{
  "name": "right-dashboard",
  "private": true,
  "type": "module",
  "scripts": {
    "typecheck": "vue-tsc --noEmit",
    "build": "vue-tsc --noEmit && vite build"
  },
  "dependencies": {
    "@vitejs/plugin-vue": "latest",
    "typescript": "latest",
    "vite": "latest",
    "vue": "latest",
    "vue-tsc": "latest"
  },
  "devDependencies": {}
}
```

Create Vite config:

```ts
export default defineConfig({
  base: './',
  plugins: [vue()],
  build: { emptyOutDir: true, outDir: '../static/dashboard', sourcemap: false },
})
```

Run:

```bash
devenv shell -- npm install --package-lock-only --prefix crates/right-dashboard/frontend
devenv shell -- npm install --prefix crates/right-dashboard/frontend
```

Expected: lockfile and local `node_modules/` are created. Request network approval if npm is blocked.

- [ ] **Step 2: Implement frontend API contract**

In `frontend/src/types.ts`, mirror the Rust DTOs:

```ts
export interface OverviewResponse { agent: string; generated_at: string; refresh_interval_secs: number; summary: OverviewSummary; crons: CronCard[]; active: ActiveActivity }
export interface OverviewSummary { cron_count: number; active_cron_count: number; failed_recent_cron_count: number; today_cost_usd: number }
export interface CronCard { job_name: string; schedule: string; recurring: boolean; run_at?: string | null; target_chat_id?: number | null; target_thread_id?: number | null; max_budget_usd: number; last_run?: RunSummary | null; recent_runs: RunSummary[] }
export interface ActiveActivity { foreground: ForegroundActivity[]; background: RunSummary[] }
export interface ForegroundActivity { chat_id: number; thread_id: number; turn_id: number }
export interface RunSummary { id: string; kind: string; producer_ref?: string | null; status: string; started_at?: string | null; finished_at?: string | null; exit_code?: number | null; delivery_status: string; cost_usd?: number | null }
export interface RunDetailResponse { run: RunSummary; summary?: string | null; notify_json?: unknown; no_notify_reason?: string | null; log: { available: boolean; path?: string | null; lines: string[]; truncated: boolean } }
```

In `frontend/src/api.ts`, send auth as:

```ts
headers: { authorization: `tma ${window.Telegram?.WebApp?.initData ?? ''}`, accept: 'application/json' }
```

- [ ] **Step 3: Implement cron-centered `App.vue`**

Build a mobile-first view with:

- top summary cards: job count, running count, recent failures, today cost;
- cron cards with schedule, status, recent runs, and cost;
- selected run detail with metadata, summary, log excerpt, and `Log unavailable`;
- compact active activity section for foreground/background counts;
- stale/offline state when polling fails;
- no write buttons.

Use Telegram theme variables such as `--tg-theme-bg-color`, `--tg-theme-text-color`, `--tg-theme-secondary-bg-color`, and `--tg-theme-hint-color`.

- [ ] **Step 4: Verify frontend and Rust asset serving**

Run:

```bash
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo check -p right-bot
```

Expected: all pass and `crates/right-dashboard/static/dashboard/` contains production assets.

- [ ] **Step 5: Commit**

Run:

```bash
devenv shell -- git add crates/right-dashboard/frontend crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "feat(dashboard): add vue cron dashboard"
```

## Task 7: Update Architecture Docs

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `docs/architecture/modules.md`
- Modify: `docs/architecture/lifecycle.md`

- [ ] **Step 1: Update `ARCHITECTURE.md`**

Change crate count to eighteen and add:

```markdown
| **right-dashboard** | `crates/right-dashboard/` | Telegram Mini App dashboard DTOs, auth validation, read models, and static assets |
```

Add the rule:

```markdown
`right-dashboard` owns Telegram Mini App dashboard DTOs, Telegram `initData`
validation, read models, and static asset lookup. `right-bot` owns runtime route
mounting, Telegram menu/button integration, allowlist injection, and bot token
injection. Dashboard writes are not exposed in the read-only MVP; future write
routes must go through bot-owned control-plane services instead of direct file
or credential edits.
```

- [ ] **Step 2: Update satellite docs**

In `docs/architecture/modules.md`, add a `right-dashboard` section covering `auth.rs`, `api_types.rs`, `read_model.rs`, `assets.rs`, `frontend/`, and `static/dashboard/`. Add `telegram/dashboard.rs` to the `right-bot` section.

In `docs/architecture/lifecycle.md`, document that `right up` emits `/dashboard/<agent>/.*` cloudflared rules and bot startup mounts `/dashboard/<agent>/` plus `/dashboard/<agent>/api/v1/*`.

- [ ] **Step 3: Verify and commit**

Run:

```bash
devenv shell -- cargo test -p right-codegen cloudflared
devenv shell -- cargo check -p right-dashboard
```

Expected: pass.

Commit:

```bash
devenv shell -- git add ARCHITECTURE.md docs/architecture/modules.md docs/architecture/lifecycle.md
devenv shell -- git commit -m "docs(architecture): document dashboard mini app"
```

## Task 8: Final Verification

**Files:**
- Inspect all touched files.

- [ ] **Step 1: Check status**

Run:

```bash
devenv shell -- git status --short
```

Expected: no tracked modifications. If `crates/right-dashboard/frontend/node_modules/` appears, add it to `.gitignore` and commit with `chore(dashboard): ignore frontend dependencies`.

- [ ] **Step 2: Run targeted checks**

Run:

```bash
devenv shell -- cargo test -p right-dashboard
devenv shell -- cargo test -p right-bot dashboard::
devenv shell -- cargo test -p right-bot dispatcher_builds_without_panic
devenv shell -- cargo test -p right-codegen cloudflared
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: all pass. Commit regenerated `static/dashboard` assets if the frontend build changes them.

- [ ] **Step 3: Run mandatory full workspace checks**

Run:

```bash
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

Expected: both pass.

- [ ] **Step 4: Final status**

Run:

```bash
devenv shell -- git status --short
devenv shell -- git log --oneline -n 10
```

Expected: no tracked modifications and implementation commits are visible.

## Self-Review Notes

- Spec coverage: this plan covers the new crate, bot-owned routes, fail-closed auth, polling APIs, cron-centered UI, Vue build, checked-in assets, `/dashboard`, menu button, cloudflared ingress, future mutation boundary, and docs.
- Scope control: no stop/retry/trigger/create/edit/delete, no MCP config UI, no budget alerts, no full raw log viewer, no WebSocket/SSE, and no separate dashboard process.
- Type consistency: Rust DTO names match TypeScript interface names; route paths consistently use `/dashboard/<agent>/api/v1/*`.
- Verification: targeted Rust tests, frontend typecheck/build, final `cargo test --workspace`, and final `cargo build --workspace` are required.
